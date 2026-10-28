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
 * The DSP has no program of its own: it comes out of reset with its PLL off
 * and its core idle, and the host loads it over I2C (on a stock unit, a
 * userspace loader at boot). openHC loads it here, entirely through the
 * kernel's stock SigmaDSP core (sound/soc/codecs/sigmadsp.c), the same one the
 * mainline ADAU1701/ADAU17x1 drivers use:
 *
 *  - The board's device tree names a firmware file ("firmware-name"). openHC
 *    builds it from board/ea/common/firmware/dsp/adau1451/: the part's
 *    control-register init, the program (built by SigmaStudio+ in CI), and the
 *    parameters, every table computed from openHC's own filter designs. No
 *    Control4 or Analog Devices file is involved.
 *  - sigmadsp writes the file's data in file order, and turns each named
 *    control into an ALSA control on the card (4 bytes, big-endian: an index
 *    the program looks up), so userspace tunes the DSP with amixer and
 *    alsa-lib, nothing custom.
 *  - At bind the part is loaded in the order of the datasheet's initialization
 *    sequence, which is also the vendor loader's: soft reset, hibernate, the
 *    register init, program and data memory, parameters, start the core, lift
 *    hibernate. The init enables the PLL part-way, and while the PLL locks the
 *    control port answers nothing, so the write that enables it waits here for
 *    PLL_LOCK before sigmadsp sends the next one.
 *
 * No safeload op. Every control in the file is a single 32-bit cell, and a
 * one-word write already lands atomically; the multi-word tables are only
 * written at bind, while the core is stopped. sigmadsp falls back to plain
 * writes when no safeload op is given.
 *
 * Then the program is identified: the device tree gives a parameter cell and
 * the value it must hold ("control4,program-version"). A loaded program that
 * does not hold it is hibernated again and the bind fails.
 *
 * A firmware file without a program (an image built before there was one)
 * holds only parameters, which mean something only to the program they were
 * made for. Then the check comes first: unless the DSP is already running that
 * program, which after reset it never is, nothing is written and the part
 * stays as it came up.
 *
 * Silent unless something plays. The DAC soft mute is held from probe and is
 * then ASoC's to move (.mute_stream): released when a stream is prepared,
 * asserted again when the last one closes — so nothing reaches the jacks at
 * power-on or while the output idles. On top of that the firmware boots the
 * program's Volume cell at 0, and ohc-audiod raises it once it has restored
 * the output's saved level and tone.
 *
 * 48 kHz only. That is the one rate the firmware lists, and the CE5300 I2S
 * this codec feeds from is fixed there too.
 *
 * Clocks: the vendor link declared this codec the I2S bit- and frame-clock
 * master (SND_SOC_DAIFMT_CBM_CFM), but the register init a stock unit loads
 * makes every serial port of the DSP a slave. The CE5300's stream comes in on
 * SDATA_IN3 with its own BCLK_IN3/LRCLK_IN3, through an ASRC, and the core
 * runs on serial input 0's frame clock (ea3.json, "init"). Nothing here
 * generates or gates clocks, and neither DAI ever implemented .set_fmt.
 */

#include <linux/delay.h>
#include <linux/firmware.h>
#include <linux/gpio/consumer.h>
#include <linux/i2c.h>
#include <linux/iopoll.h>
#include <linux/mod_devicetable.h>
#include <linux/module.h>
#include <linux/property.h>
#include <linux/regmap.h>
#include <linux/unaligned.h>
#include <sound/pcm.h>
#include <sound/soc.h>

#include "sigmadsp.h"

/*
 * ADAU145x core control registers, from the public ADAU1450/1451/1452
 * datasheet register map.
 */
#define ADAU1451_REG_PLL_ENABLE		0xf003
#define ADAU1451_REG_PLL_LOCK		0xf004
#define ADAU1451_REG_HIBERNATE		0xf400
#define ADAU1451_REG_START_CORE		0xf402
#define ADAU1451_REG_CORE_STATUS	0xf405
#define ADAU1451_REG_SOFT_RESET		0xf890

#define ADAU1451_PARAM_MAXREG		0x9fff	/* DM0 from 0x0000, DM1 to 0x9fff */
#define ADAU1451_CTRL_MAXREG		0xf8ff
#define ADAU1451_PROGRAM_BASE		0xc000	/* program memory, 8192 words */
#define ADAU1451_PROGRAM_END		0xe000

#define ADAU1451_RESET_DELAY_MS		255	/* vendor's numbers, not the datasheet's */
#define ADAU1451_HIBERNATE_DELAY_MS	255
#define ADAU1451_PLL_LOCK_US		50000	/* datasheet worst case: 10.666 ms */
#define ADAU1451_RATE			48000

/* The SigmaDSP firmware file: header, then {le32 length, le32 tag, le32 rates}. */
#define ADAU1451_FW_HEADER		12
#define ADAU1451_FW_CHUNK		12
#define ADAU1451_FW_CHUNK_DATA		0	/* then le16 address, the bytes */

struct adau1451_c4 {
	struct regmap *param;		/* parameter RAM, 32-bit words */
	struct regmap *ctrl;		/* control registers, 16-bit words */
	struct sigmadsp *dsp;		/* NULL: leave the DSP as it came up */
	bool load;			/* the firmware carries a program */
	int (*write)(void *, unsigned int, const u8 *, size_t);	/* sigmadsp's */
	struct gpio_desc *mute_gpio;	/* DAC soft mute, asserted = muted */
};

/*
 * Parameter RAM: 16-bit address, 32-bit big-endian value. No cache: sigmadsp
 * keeps its own copy of what it wrote, and this map is only read.
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

static int adau1451_c4_hibernate(struct adau1451_c4 *st)
{
	int ret;

	/* The paired 0-then-1 writes are edge triggers. */
	ret = regmap_write(st->ctrl, ADAU1451_REG_HIBERNATE, 0) ?:
	      regmap_write(st->ctrl, ADAU1451_REG_HIBERNATE, 1);
	if (ret)
		return ret;

	msleep(ADAU1451_HIBERNATE_DELAY_MS);
	return 0;
}

/*
 * START_CORE is edge triggered, hence 0 then 1. Hibernation masks the core's
 * interrupts, so it has to be lifted too: the vendor's loader does that last,
 * after the start, which is why a stock unit reads HIBERNATE 0 mid-play.
 */
static int adau1451_c4_start(struct adau1451_c4 *st)
{
	int ret;

	ret = regmap_write(st->ctrl, ADAU1451_REG_START_CORE, 0) ?:
	      regmap_write(st->ctrl, ADAU1451_REG_START_CORE, 1);
	if (ret)
		return ret;

	usleep_range(1000, 2000);
	return regmap_write(st->ctrl, ADAU1451_REG_HIBERNATE, 0);
}

/* A failed read is "not yet": the control port NAKs until the PLL locks. */
static bool adau1451_c4_pll_locked(struct adau1451_c4 *st)
{
	unsigned int val;

	return !regmap_read(st->ctrl, ADAU1451_REG_PLL_LOCK, &val) && (val & 1);
}

/*
 * sigmadsp's own I2C write, plus the wait for the PLL: after the write that
 * enables it, nothing reaches the part until it has locked.
 */
static int adau1451_c4_write(void *control_data, unsigned int addr,
			     const u8 *data, size_t len)
{
	struct i2c_client *client = control_data;
	struct adau1451_c4 *st = i2c_get_clientdata(client);
	bool locked;
	int ret;

	ret = st->write(control_data, addr, data, len);
	if (ret || addr != ADAU1451_REG_PLL_ENABLE || len != 2 || !(data[1] & 1))
		return ret;

	ret = read_poll_timeout(adau1451_c4_pll_locked, locked, locked, 1000,
				ADAU1451_PLL_LOCK_US, true, st);
	if (ret)
		dev_err(&client->dev, "PLL did not lock: is MCLK running?\n");
	return ret;
}

/*
 * Is the running program the one the firmware describes? The device tree
 * names a cell and the value it must hold. 1: it is; 0: it is not, or the
 * device tree does not say; negative: the read failed.
 */
static int adau1451_c4_program_ok(struct adau1451_c4 *st, struct device *dev)
{
	u32 want[2];
	unsigned int got;
	int ret;

	if (device_property_read_u32_array(dev, "control4,program-version",
					   want, ARRAY_SIZE(want))) {
		dev_warn(dev, "no control4,program-version: cannot tell which program is running\n");
		return 0;
	}
	ret = regmap_read(st->param, want[0], &got);
	if (ret)
		return ret;
	if (got != want[1]) {
		dev_warn(dev, "program version cell 0x%04x reads %u, not %u\n",
			 want[0], got, want[1]);
		return 0;
	}
	return 1;
}

/*
 * Load the part from the firmware file, program and all. Any failure leaves
 * the core stopped and fails the bind: a half-loaded program never runs.
 */
static int adau1451_c4_load(struct adau1451_c4 *st, struct device *dev)
{
	unsigned int status;
	int ret;

	/*
	 * Every control register back to its default, except the PLL's, then
	 * the vendor's pause after it.
	 */
	ret = regmap_write(st->ctrl, ADAU1451_REG_SOFT_RESET, 0) ?:
	      regmap_write(st->ctrl, ADAU1451_REG_SOFT_RESET, 1);
	if (ret)
		return ret;
	msleep(ADAU1451_RESET_DELAY_MS);

	ret = adau1451_c4_hibernate(st);
	if (ret)
		return ret;

	/*
	 * The register init, program and data memory, then the parameters: the
	 * file, in order. Reset first, or a rebind would find the rate already
	 * set up and write nothing after the soft reset.
	 */
	sigmadsp_reset(st->dsp);
	ret = sigmadsp_setup(st->dsp, ADAU1451_RATE);
	if (ret) {
		dev_err(dev, "loading the DSP failed: %d\n", ret);
		return ret;
	}

	ret = adau1451_c4_start(st);
	if (ret)
		return ret;

	ret = adau1451_c4_program_ok(st, dev);
	if (ret <= 0) {
		if (!ret)
			dev_err(dev, "the loaded program is not the one the device tree names: stopping it\n");
		adau1451_c4_hibernate(st);
		return ret ?: -EINVAL;
	}

	if (!regmap_read(st->ctrl, ADAU1451_REG_CORE_STATUS, &status))
		dev_info(dev, "DSP program loaded, core status %u\n", status);
	return 0;
}

/*
 * Bind: hand the firmware's controls to the card, then load the part, or,
 * with no program in the file, fill parameter RAM with the core stopped and
 * start it again. The DAC stays muted throughout; only a stream releases it
 * (adau1451_c4_mute_stream).
 */
static int adau1451_c4_component_probe(struct snd_soc_component *comp)
{
	struct adau1451_c4 *st = snd_soc_component_get_drvdata(comp);
	int ret;

	if (!st->dsp)
		return 0;

	ret = sigmadsp_attach(st->dsp, comp);
	if (ret)
		return ret;

	if (st->load)
		return adau1451_c4_load(st, comp->dev);

	ret = adau1451_c4_hibernate(st);
	if (ret)
		return ret;
	/* Not fatal: the core restarts on whatever made it in, as it booted. */
	ret = sigmadsp_setup(st->dsp, ADAU1451_RATE);
	if (ret)
		dev_err(comp->dev, "writing the DSP parameters failed: %d\n", ret);
	return adau1451_c4_start(st);
}

static void adau1451_c4_component_remove(struct snd_soc_component *comp)
{
	struct adau1451_c4 *st = snd_soc_component_get_drvdata(comp);

	gpiod_set_value_cansleep(st->mute_gpio, 1);
}

static int adau1451_c4_startup(struct snd_pcm_substream *substream,
			       struct snd_soc_dai *dai)
{
	struct adau1451_c4 *st = snd_soc_component_get_drvdata(dai->component);

	return st->dsp ? sigmadsp_restrict_params(st->dsp, substream) : 0;
}

/* The DAC soft mute follows the stream: open while one plays, shut otherwise. */
static int adau1451_c4_mute_stream(struct snd_soc_dai *dai, int mute, int stream)
{
	struct adau1451_c4 *st = snd_soc_component_get_drvdata(dai->component);

	gpiod_set_value_cansleep(st->mute_gpio, mute);
	return 0;
}

static const struct snd_soc_dai_ops adau1451_c4_dai_ops = {
	.startup	= adau1451_c4_startup,
	.mute_stream	= adau1451_c4_mute_stream,
	.no_capture_mute = 1,
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
	.probe		= adau1451_c4_component_probe,
	.remove		= adau1451_c4_component_remove,
};

/*
 * Does the firmware carry a program? sigmadsp keeps what it parsed to itself,
 * so walk the file's chunks (sigmadsp has checked it already) for data bound
 * for program memory.
 */
static bool adau1451_c4_fw_has_program(struct device *dev, const char *name)
{
	const struct firmware *fw;
	size_t pos = ADAU1451_FW_HEADER;
	bool found = false;

	if (request_firmware(&fw, name, dev))
		return false;

	while (!found && pos + ADAU1451_FW_CHUNK + 2 < fw->size) {
		const u8 *chunk = fw->data + pos;
		u32 len = get_unaligned_le32(chunk);
		unsigned int addr = get_unaligned_le16(chunk + ADAU1451_FW_CHUNK);

		if (len < ADAU1451_FW_CHUNK || len > fw->size - pos)
			break;
		found = get_unaligned_le32(chunk + 4) == ADAU1451_FW_CHUNK_DATA &&
			addr >= ADAU1451_PROGRAM_BASE && addr < ADAU1451_PROGRAM_END;
		pos += ALIGN(len, 4);
	}

	release_firmware(fw);
	return found;
}

/*
 * Take the firmware file to sigmadsp. A missing or bad one is not fatal:
 * nothing is written and the DSP stays as it came up, there are just no DSP
 * controls.
 */
static int adau1451_c4_init_dsp(struct adau1451_c4 *st,
				struct i2c_client *client, const char *fw)
{
	struct device *dev = &client->dev;
	int ret;

	st->dsp = devm_sigmadsp_init_i2c(client, NULL, fw);
	if (IS_ERR(st->dsp)) {
		dev_warn(dev, "no DSP firmware from %s (%ld): leaving the DSP as it came up\n",
			 fw, PTR_ERR(st->dsp));
		st->dsp = NULL;
		return 0;
	}

	if (adau1451_c4_fw_has_program(dev, fw)) {
		/* Ours to load, through the PLL wait. */
		st->load = true;
		st->write = st->dsp->write;
		st->dsp->write = adau1451_c4_write;
		return 0;
	}

	/* Parameters only: for a program that must already be running. */
	ret = adau1451_c4_program_ok(st, dev);
	if (ret < 0)
		return ret;
	if (!ret) {
		dev_warn(dev, "%s has no program, and the DSP is not running the one its parameters are for: leaving it as it came up\n",
			 fw);
		st->dsp = NULL;
	}
	return 0;
}

static int adau1451_c4_i2c_probe(struct i2c_client *client)
{
	struct device *dev = &client->dev;
	struct adau1451_c4 *st;
	unsigned int val;
	const char *fw;
	int ret;

	st = devm_kzalloc(dev, sizeof(*st), GFP_KERNEL);
	if (!st)
		return -ENOMEM;

	/*
	 * Optional: the line is a board-level DAC soft mute (vendor global GPIO
	 * 61), not part of the codec, and probe must still succeed without it so
	 * the codec can be brought up before the GPIO controller is sorted out.
	 * Start asserted — nothing is configured yet.
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

	/*
	 * The liveness check: a codec that is absent, or on another i2c bus
	 * than we think, NAKs here at probe instead of turning into silence much
	 * later.
	 */
	ret = regmap_read(st->param, 0, &val);
	if (ret)
		return dev_err_probe(dev, ret, "DSP did not respond\n");

	if (!device_property_read_string(dev, "firmware-name", &fw)) {
		ret = adau1451_c4_init_dsp(st, client, fw);
		if (ret)
			return ret;
	}

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
