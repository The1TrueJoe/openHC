// SPDX-License-Identifier: GPL-2.0
/*
 * ASoC codec driver for the ADAU1451 SigmaDSP on Control4 EA-series boards
 * (EA-3 / Intel CE5300, codec at i2c 0x38).
 *
 * Derived from Control4's GPL kernel drop — sound/soc/codecs/adau1451.c and
 * adau1451-common.c, both headed "Licensed under the GPL-2 or later" — which
 * were in turn derived from the Analog Devices ADAU1701 driver by
 * Lars-Peter Clausen <lars@metafoo.de>.
 *
 * Reduced to PCM playback at 48 kHz, deliberately:
 *
 *  - No firmware, no sigmadsp. The stock vendor driver pre-seeded the DSP's
 *    "current samplerate" to 48000, which makes its sigmadsp_reload() return
 *    immediately, and its program-memory loader was #if 0'd out. So at 48 kHz
 *    nothing from ea3-1451.bin was ever written: the part self-boots its own
 *    program from EEPROM and Linux only ever pushed parameter values. Dropping
 *    request_firmware is therefore not a regression at 48 kHz, and it is what
 *    lets openHC ship with zero Control4/ADI binaries.
 *
 *  - No tone controls. Every mixer control the vendor exposed (volume, mute,
 *    bass, treble, balance, loudness, a 10-band EQ, input gain, output mux)
 *    addresses parameter RAM through ea3-dsp-params.h, an Analog Devices
 *    SigmaStudio export that may not be redistributed. See the TODO below.
 *
 *  - No rate switching. 44.1/96/192 kHz all went through a DSP hibernate +
 *    PARAM reload out of that same firmware file. Without it the DSP would be
 *    hibernated, given nothing, and restarted — silence or noise. 48 kHz only.
 *
 * The codec is the I2S bit- and frame-clock master (the vendor link used
 * SND_SOC_DAIFMT_CBM_CFM); the CE5300 I2S block is the slave. Nothing here
 * generates or gates clocks, and neither DAI ever implemented .set_fmt.
 */

#include <linux/delay.h>
#include <linux/gpio/consumer.h>
#include <linux/i2c.h>
#include <linux/mod_devicetable.h>
#include <linux/module.h>
#include <linux/regmap.h>
#include <sound/pcm.h>
#include <sound/soc.h>

/*
 * ADAU145x hardware registers, from the public ADAU1450/1451/1452 datasheet
 * register map — not from the vendor's SigmaStudio parameter header.
 */
#define ADAU1451_REG_SAFELOAD_DATA0	0x0018	/* 5 slots, 0x18..0x1c */
#define ADAU1451_REG_SAFELOAD_ADDR	0x001d
#define ADAU1451_REG_SAFELOAD_NUM	0x001e
#define ADAU1451_SAFELOAD_SLOTS		5

#define ADAU1451_REG_HIBERNATE		0xf400
#define ADAU1451_REG_START_CORE		0xf402

#define ADAU1451_PARAM_MAXREG		0x1fff	/* vendor's ADAU1451_MAXREG */
#define ADAU1451_CTRL_MAXREG		0xf800

#define ADAU1451_HIBERNATE_DELAY_MS	255	/* vendor's number, not the datasheet's */

struct adau1451_c4 {
	struct regmap *param;		/* parameter RAM, 32-bit words */
	struct regmap *ctrl;		/* 0xf4xx core control, 16-bit words */
	struct gpio_desc *mute_gpio;	/* DAC soft mute, asserted = muted */
};

/*
 * Parameter RAM: 16-bit address, 32-bit big-endian value. This is the same
 * wire format the vendor's hand-rolled i2c write used, so regmap can carry it.
 *
 * No reg_defaults and no cache, unlike the vendor config. Its cache existed
 * only to hold the defaults from ea3-dsp-params.h and replay them after a DSP
 * restart; with no defaults there is nothing to replay, and a cache would
 * actively break safeload — the trigger write to SAFELOAD_NUM must reach the
 * part every time, even when the value repeats.
 */
static const struct regmap_config adau1451_c4_param_regmap = {
	.name		= "param",
	.reg_bits	= 16,
	.val_bits	= 32,
	.max_register	= ADAU1451_PARAM_MAXREG,
	.cache_type	= REGCACHE_NONE,
};

/*
 * Core control registers are 16-bit values and live above the parameter RAM
 * window, so they need their own map. The vendor reached around regmap with
 * raw i2c_master_send() for exactly this reason.
 */
static const struct regmap_config adau1451_c4_ctrl_regmap = {
	.name		= "ctrl",
	.reg_bits	= 16,
	.val_bits	= 16,
	.max_register	= ADAU1451_CTRL_MAXREG,
	.cache_type	= REGCACHE_NONE,
};

/*
 * Safeload: stage up to 5 words, point at a target address, then trigger. The
 * DSP applies the whole block between sample periods so a multi-word parameter
 * never goes out half-updated.
 *
 * Currently unreferenced: every caller the vendor had was a mixer control at an
 * ea3-dsp-params.h address (see the TODO in the file header). Kept because it
 * is the only correct way to write parameter RAM on a running core, and it is
 * what any restored control will need.
 */
static int __maybe_unused adau1451_c4_safeload_write(struct adau1451_c4 *st,
						     unsigned int addr,
						     const u32 *data, size_t num)
{
	int ret;
	size_t i;

	if (num >= ADAU1451_SAFELOAD_SLOTS)
		return -EINVAL;

	for (i = 0; i < num; i++) {
		ret = regmap_write(st->param, ADAU1451_REG_SAFELOAD_DATA0 + i,
				   data[i]);
		if (ret)
			return ret;
	}

	/* address before count: writing the count is what fires the load */
	ret = regmap_write(st->param, ADAU1451_REG_SAFELOAD_ADDR, addr);
	if (ret)
		return ret;

	return regmap_write(st->param, ADAU1451_REG_SAFELOAD_NUM, num);
}

/*
 * Hibernate the core, wait, then pulse START_CORE. Verbatim from the vendor
 * (adau1451-common.c:154-174) minus the parameter reload that used to sit in
 * the middle. The paired 0-then-1 writes are edge triggers.
 *
 * Two things here look wrong and are left alone because this is the sequence
 * that ships on working hardware: the 255 ms is a magic vendor number, and
 * HIBERNATE is left reading 1 at the end.
 *
 * The first write doubles as our only liveness check — a codec that is absent,
 * or on a different i2c bus than we think, NAKs here at probe instead of
 * turning into silence much later.
 */
static int adau1451_c4_dsp_restart(struct adau1451_c4 *st)
{
	int ret;

	ret = regmap_write(st->ctrl, ADAU1451_REG_HIBERNATE, 0) ?:
	      regmap_write(st->ctrl, ADAU1451_REG_HIBERNATE, 1);
	if (ret)
		return ret;

	msleep(ADAU1451_HIBERNATE_DELAY_MS);

	ret = regmap_write(st->ctrl, ADAU1451_REG_START_CORE, 0) ?:
	      regmap_write(st->ctrl, ADAU1451_REG_START_CORE, 1);
	if (ret)
		return ret;

	usleep_range(1000, 2000);

	return 0;
}

static int adau1451_c4_startup(struct snd_pcm_substream *substream,
			       struct snd_soc_dai *dai)
{
	struct adau1451_c4 *st = snd_soc_component_get_drvdata(dai->component);

	gpiod_set_value_cansleep(st->mute_gpio, 0);

	return 0;
}

static void adau1451_c4_shutdown(struct snd_pcm_substream *substream,
				 struct snd_soc_dai *dai)
{
	struct adau1451_c4 *st = snd_soc_component_get_drvdata(dai->component);

	gpiod_set_value_cansleep(st->mute_gpio, 1);
}

static const struct snd_soc_dai_ops adau1451_c4_dai_ops = {
	.startup	= adau1451_c4_startup,
	.shutdown	= adau1451_c4_shutdown,
};

/*
 * Stereo only. The vendor advertised 2..8 channels, but the extra slots only
 * meant anything to a SigmaStudio program routing them, and the CE5300 render
 * path we drive is stereo. S16_LE and S32_LE: the serial input port clocks
 * 32-bit slots and ignores the unused LSBs, so both land the same way; they are
 * two of the four formats the vendor DAI listed. S24_3LE/S24_LE are dropped
 * only because nothing needs them yet.
 */
static struct snd_soc_dai_driver adau1451_c4_dai = {
	.name = "adau1451-ch0",
	.playback = {
		.stream_name	= "Playback",
		.channels_min	= 2,
		.channels_max	= 2,
		.rates		= SNDRV_PCM_RATE_48000,
		.formats	= SNDRV_PCM_FMTBIT_S16_LE |
				  SNDRV_PCM_FMTBIT_S32_LE,
	},
	.ops = &adau1451_c4_dai_ops,
};

static const struct snd_soc_component_driver adau1451_c4_component = {
	/*
	 * No controls, no DAPM widgets, no routes — the vendor had none on this
	 * path either, and every control it did have needs an address we cannot
	 * ship. Nothing to set here yet.
	 *
	 * TODO: restoring Master Playback Volume/Switch, bass, treble, balance,
	 * loudness, the 10-band EQ, input gain and the output mux needs ~41
	 * parameter-RAM addresses from Analog Devices' SigmaStudio export for
	 * the "SamX Rev 10 48K" program. That header is not redistributable, so
	 * the addresses must be supplied out of band (or regenerated from our
	 * own SigmaStudio project). Do not guess them: a wrong address writes
	 * into a live DSP program. Until then ALSA softvol covers volume.
	 */
};

static int adau1451_c4_i2c_probe(struct i2c_client *client)
{
	struct device *dev = &client->dev;
	struct adau1451_c4 *st;
	int ret;

	st = devm_kzalloc(dev, sizeof(*st), GFP_KERNEL);
	if (!st)
		return -ENOMEM;

	/*
	 * Optional: the line is a board-level DAC soft mute (vendor global GPIO
	 * 61), not part of the codec, and probe must still succeed without it so
	 * the codec can be brought up before the GPIO controller is sorted out.
	 * Start asserted — nothing is playing yet.
	 */
	st->mute_gpio = devm_gpiod_get_optional(dev, "mute", GPIOD_OUT_HIGH);
	if (IS_ERR(st->mute_gpio))
		return dev_err_probe(dev, PTR_ERR(st->mute_gpio),
				     "failed to get DAC mute gpio\n");

	st->param = devm_regmap_init_i2c(client, &adau1451_c4_param_regmap);
	if (IS_ERR(st->param))
		return dev_err_probe(dev, PTR_ERR(st->param),
				     "param regmap init failed\n");

	st->ctrl = devm_regmap_init_i2c(client, &adau1451_c4_ctrl_regmap);
	if (IS_ERR(st->ctrl))
		return dev_err_probe(dev, PTR_ERR(st->ctrl),
				     "ctrl regmap init failed\n");

	i2c_set_clientdata(client, st);

	ret = adau1451_c4_dsp_restart(st);
	if (ret)
		return dev_err_probe(dev, ret, "DSP did not respond\n");

	return devm_snd_soc_register_component(dev, &adau1451_c4_component,
					       &adau1451_c4_dai, 1);
}

static const struct i2c_device_id adau1451_c4_i2c_id[] = {
	{ "adau1450" },
	{ "adau1451" },
	{ "adau1452" },
	{ }
};
MODULE_DEVICE_TABLE(i2c, adau1451_c4_i2c_id);

static const struct of_device_id adau1451_c4_of_match[] = {
	{ .compatible = "adi,adau1451" },
	{ }
};
MODULE_DEVICE_TABLE(of, adau1451_c4_of_match);

static struct i2c_driver adau1451_c4_i2c_driver = {
	.driver = {
		.name		= "adau1451",
		.of_match_table	= adau1451_c4_of_match,
	},
	.probe		= adau1451_c4_i2c_probe,
	.id_table	= adau1451_c4_i2c_id,
};
module_i2c_driver(adau1451_c4_i2c_driver);

MODULE_DESCRIPTION("ASoC ADAU1451 SigmaDSP driver for Control4 EA hardware");
MODULE_AUTHOR("Control4 Corporation");
MODULE_LICENSE("GPL");
