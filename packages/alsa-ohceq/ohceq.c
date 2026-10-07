/*
 * ohceq — openHC's per-output tone stage, as an ALSA filter plugin.
 *
 * Bass and treble (RBJ shelving biquads at 120 Hz and 8 kHz, +-12 dB) and
 * left/right balance, on 16-bit stereo. ohc-audiod puts one in front of every
 * output, between its level (softvol) and its dmix:
 *
 *   pcm.ohc_analog2_eq {
 *       type ohceq
 *       slave.pcm "ohc_analog2_dmix"
 *       card "Intel"
 *       control "ohc_analog2"
 *   }
 *
 * Like softvol, it creates its own mixer controls on the card the first time
 * it is opened, and reads them as it plays, so a change applies live without
 * reopening anything:
 *
 *   "<control> Bass Playback Volume"     -12..12 dB
 *   "<control> Treble Playback Volume"   -12..12 dB
 *   "<control> Balance Playback Volume"  -100 (left only)..100 (right only)
 *
 * Boosting would clip a full-scale signal, so the input is pre-attenuated by the
 * largest boost (a +6 dB bass shelf costs 6 dB of headroom, not distortion).
 * Flat and centred, it copies samples straight through.
 */
#include <alsa/asoundlib.h>
#include <alsa/pcm_external.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

#define BASS_HZ 120.0
#define TREBLE_HZ 8000.0
#define TONE_MAX_DB 12
#define BALANCE_MAX 100
/* Re-read the controls about every 20 ms of audio at 44.1/48 kHz. */
#define CHECK_FRAMES 1024

enum { BASS, TREBLE, BALANCE, NCTL };
static const char *const SUFFIX[NCTL] = { "Bass", "Treble", "Balance" };

typedef struct {
	float b0, b1, b2, a1, a2;
} biquad_t;

typedef struct {
	snd_pcm_extplug_t ext;
	snd_ctl_t *ctl;
	snd_ctl_elem_value_t *val[NCTL];
	int cur[NCTL];              /* the settings the filters were built for */
	int have[NCTL];             /* whether each control could be found */
	biquad_t shelf[2];          /* bass, treble */
	float z[2][2][2];           /* [filter][channel][state] */
	float pre, gain[2];
	int bypass;
	unsigned rate;
	snd_pcm_uframes_t since_check;
} ohceq_t;

/* RBJ audio-EQ cookbook shelves, shelf slope S = 1. */
static biquad_t shelf(int high, double db, double f0, double fs)
{
	double A = pow(10.0, db / 40.0), w0 = 2.0 * M_PI * f0 / fs;
	double c = cos(w0), alpha = sin(w0) / 2.0 * sqrt(2.0), sa = 2.0 * sqrt(A) * alpha;
	double b0, b1, b2, a0, a1, a2;
	if (!high) {
		b0 = A * ((A + 1) - (A - 1) * c + sa);
		b1 = 2 * A * ((A - 1) - (A + 1) * c);
		b2 = A * ((A + 1) - (A - 1) * c - sa);
		a0 = (A + 1) + (A - 1) * c + sa;
		a1 = -2 * ((A - 1) + (A + 1) * c);
		a2 = (A + 1) + (A - 1) * c - sa;
	} else {
		b0 = A * ((A + 1) + (A - 1) * c + sa);
		b1 = -2 * A * ((A - 1) + (A + 1) * c);
		b2 = A * ((A + 1) + (A - 1) * c - sa);
		a0 = (A + 1) - (A - 1) * c + sa;
		a1 = 2 * ((A - 1) - (A + 1) * c);
		a2 = (A + 1) - (A - 1) * c - sa;
	}
	biquad_t q = { (float)(b0 / a0), (float)(b1 / a0), (float)(b2 / a0), (float)(a1 / a0), (float)(a2 / a0) };
	return q;
}

static void rebuild(ohceq_t *eq)
{
	int bass = eq->cur[BASS], treble = eq->cur[TREBLE], bal = eq->cur[BALANCE];
	unsigned fs = eq->rate ? eq->rate : 44100;
	eq->shelf[0] = shelf(0, bass, BASS_HZ, fs);
	eq->shelf[1] = shelf(1, treble, TREBLE_HZ, fs);
	int boost = bass > treble ? bass : treble;
	eq->pre = boost > 0 ? (float)pow(10.0, -boost / 20.0) : 1.0f;
	eq->gain[0] = bal > 0 ? 1.0f - bal / (float)BALANCE_MAX : 1.0f;
	eq->gain[1] = bal < 0 ? 1.0f + bal / (float)BALANCE_MAX : 1.0f;
	eq->bypass = bass == 0 && treble == 0 && bal == 0;
}

/* Read the controls; rebuild the filters if anything moved. */
static void check(ohceq_t *eq)
{
	int moved = 0;
	for (int i = 0; i < NCTL; i++) {
		if (!eq->have[i] || snd_ctl_elem_read(eq->ctl, eq->val[i]) < 0)
			continue;
		int v = (int)snd_ctl_elem_value_get_integer(eq->val[i], 0);
		if (v != eq->cur[i]) {
			eq->cur[i] = v;
			moved = 1;
		}
	}
	if (moved)
		rebuild(eq);
}

static inline short clip(float v)
{
	if (v > 32767.0f) return 32767;
	if (v < -32768.0f) return -32768;
	return (short)lrintf(v);
}

static snd_pcm_sframes_t ohceq_transfer(snd_pcm_extplug_t *ext,
					const snd_pcm_channel_area_t *dst, snd_pcm_uframes_t doff,
					const snd_pcm_channel_area_t *src, snd_pcm_uframes_t soff,
					snd_pcm_uframes_t size)
{
	ohceq_t *eq = ext->private_data;
	eq->since_check += size;
	if (eq->since_check >= CHECK_FRAMES) {
		eq->since_check = 0;
		check(eq);
	}
	for (int ch = 0; ch < 2; ch++) {
		const short *in = (const short *)((const char *)src[ch].addr + (src[ch].first + soff * src[ch].step) / 8);
		short *out = (short *)((char *)dst[ch].addr + (dst[ch].first + doff * dst[ch].step) / 8);
		unsigned is = src[ch].step / 16, os = dst[ch].step / 16;
		if (eq->bypass) {
			for (snd_pcm_uframes_t n = 0; n < size; n++)
				out[n * os] = in[n * is];
			continue;
		}
		float g = eq->pre * eq->gain[ch];
		for (snd_pcm_uframes_t n = 0; n < size; n++) {
			float x = in[n * is] * g;
			for (int f = 0; f < 2; f++) {
				const biquad_t *q = &eq->shelf[f];
				float *z = eq->z[f][ch];
				float y = q->b0 * x + z[0];      /* transposed direct form II */
				z[0] = q->b1 * x - q->a1 * y + z[1];
				z[1] = q->b2 * x - q->a2 * y;
				x = y;
			}
			out[n * os] = clip(x);
		}
	}
	return size;
}

static int ohceq_init(snd_pcm_extplug_t *ext)
{
	ohceq_t *eq = ext->private_data;
	eq->rate = ext->rate;
	memset(eq->z, 0, sizeof(eq->z));
	check(eq);
	rebuild(eq);
	return 0;
}

static int ohceq_close(snd_pcm_extplug_t *ext)
{
	ohceq_t *eq = ext->private_data;
	for (int i = 0; i < NCTL; i++)
		if (eq->val[i])
			snd_ctl_elem_value_free(eq->val[i]);
	if (eq->ctl)
		snd_ctl_close(eq->ctl);
	free(eq);
	return 0;
}

static const snd_pcm_extplug_callback_t ohceq_callback = {
	.transfer = ohceq_transfer,
	.init = ohceq_init,
	.close = ohceq_close,
};

/* Find "<base> <suffix> Playback Volume", creating it (at 0) if it is new. */
static int control(ohceq_t *eq, int i, const char *base)
{
	char name[64];
	snd_ctl_elem_id_t *id;
	snd_ctl_elem_info_t *info;
	snd_ctl_elem_id_alloca(&id);
	snd_ctl_elem_info_alloca(&info);
	snprintf(name, sizeof(name), "%s %s Playback Volume", base, SUFFIX[i]);
	snd_ctl_elem_id_set_interface(id, SND_CTL_ELEM_IFACE_MIXER);
	snd_ctl_elem_id_set_name(id, name);
	snd_ctl_elem_info_set_id(info, id);
	if (snd_ctl_elem_info(eq->ctl, info) < 0) {
		long lim = i == BALANCE ? BALANCE_MAX : TONE_MAX_DB;
		int err = snd_ctl_elem_add_integer(eq->ctl, id, 1, -lim, lim, 1);
		if (err < 0 && err != -EBUSY) {
			SNDERR("ohceq: cannot add control '%s': %s", name, snd_strerror(err));
			return err;
		}
		if (i != BALANCE) {
			/* dB scale for amixer and friends: min -12.00 dB, 1.00 dB a step. */
			unsigned int tlv[4] = { SND_CTL_TLVT_DB_SCALE, 2 * sizeof(unsigned int),
						(unsigned int)(-TONE_MAX_DB * 100), 100 };
			snd_ctl_elem_tlv_write(eq->ctl, id, tlv);
		}
		snd_ctl_elem_value_t *zero;
		snd_ctl_elem_value_alloca(&zero);
		snd_ctl_elem_value_set_id(zero, id);
		snd_ctl_elem_value_set_integer(zero, 0, 0);
		snd_ctl_elem_write(eq->ctl, zero);
	}
	if (snd_ctl_elem_value_malloc(&eq->val[i]) < 0)
		return -ENOMEM;
	snd_ctl_elem_value_set_id(eq->val[i], id);
	eq->have[i] = 1;
	return 0;
}

SND_PCM_PLUGIN_DEFINE_FUNC(ohceq)
{
	snd_config_iterator_t i, next;
	snd_config_t *slave = NULL;
	const char *card = "0", *base = NULL;
	int err;

	snd_config_for_each(i, next, conf) {
		snd_config_t *n = snd_config_iterator_entry(i);
		const char *id;
		if (snd_config_get_id(n, &id) < 0)
			continue;
		if (!strcmp(id, "comment") || !strcmp(id, "type") || !strcmp(id, "hint"))
			continue;
		if (!strcmp(id, "slave")) {
			slave = n;
			continue;
		}
		if (!strcmp(id, "card")) {
			if (snd_config_get_string(n, &card) < 0) {
				SNDERR("ohceq: card must be a string");
				return -EINVAL;
			}
			continue;
		}
		if (!strcmp(id, "control")) {
			if (snd_config_get_string(n, &base) < 0) {
				SNDERR("ohceq: control must be a string");
				return -EINVAL;
			}
			continue;
		}
		SNDERR("ohceq: unknown field %s", id);
		return -EINVAL;
	}
	if (!slave || !base) {
		SNDERR("ohceq: needs slave and control");
		return -EINVAL;
	}

	ohceq_t *eq = calloc(1, sizeof(*eq));
	if (!eq)
		return -ENOMEM;
	char dev[64];
	snprintf(dev, sizeof(dev), "hw:%s", card);
	if (snd_ctl_open(&eq->ctl, dev, 0) == 0) {
		for (int c = 0; c < NCTL; c++)
			control(eq, c, base); /* a missing control just means "flat" */
	} else {
		SNDERR("ohceq: cannot open %s; tone stays flat", dev);
		eq->ctl = NULL;
	}
	rebuild(eq);

	eq->ext.version = SND_PCM_EXTPLUG_VERSION;
	eq->ext.name = "openHC tone (bass/treble/balance)";
	eq->ext.callback = &ohceq_callback;
	eq->ext.private_data = eq;
	err = snd_pcm_extplug_create(&eq->ext, name, root, slave, stream, mode);
	if (err < 0) {
		if (eq->ctl)
			snd_ctl_close(eq->ctl);
		free(eq);
		return err;
	}
	snd_pcm_extplug_set_param_minmax(&eq->ext, SND_PCM_EXTPLUG_HW_CHANNELS, 2, 2);
	snd_pcm_extplug_set_slave_param(&eq->ext, SND_PCM_EXTPLUG_HW_CHANNELS, 2);
	snd_pcm_extplug_set_param(&eq->ext, SND_PCM_EXTPLUG_HW_FORMAT, SND_PCM_FORMAT_S16);
	snd_pcm_extplug_set_slave_param(&eq->ext, SND_PCM_EXTPLUG_HW_FORMAT, SND_PCM_FORMAT_S16);
	*pcmp = eq->ext.pcm;
	return 0;
}

SND_PCM_PLUGIN_SYMBOL(ohceq);
