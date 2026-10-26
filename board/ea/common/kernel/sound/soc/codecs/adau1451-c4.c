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
 * The DSP boots its own program from the board's EEPROM; nothing here loads
 * program memory. What openHC owns is the PARAMETER RAM that program reads,
 * and it reaches it entirely through the kernel's stock SigmaDSP core
 * (sound/soc/codecs/sigmadsp.c), the same one the mainline ADAU1701/ADAU17x1
 * drivers use:
 *
 *  - The board's device tree names a firmware file ("firmware-name"). openHC
 *    builds it from board/ea/common/firmware/dsp/adau1451/ — every table in it
 *    is computed from openHC's own filter designs, and every address was read
 *    off a running unit. No Control4 or Analog Devices file is involved.
 *  - sigmadsp turns each named control in that file into an ALSA control on
 *    the card (4 bytes, big-endian: an index the program looks up), so
 *    userspace tunes the DSP with amixer and alsa-lib, nothing custom.
 *  - At bind the file's parameter data goes in with the core hibernated —
 *    the vendor's own order (adau1451-common.c reset: hibernate, reload,
 *    start) — so the program never runs on a half-written table.
 *
 * No safeload op. Every control in the file is a single 32-bit cell, and a
 * one-word write already lands atomically; the multi-word tables are only
 * written at bind, while the core is stopped. sigmadsp falls back to plain
 * writes when no safeload op is given.
 *
 * Before any of that, the program is identified: the device tree gives a
 * parameter cell and the value it must hold ("control4,program-version").
 * The firmware's addresses mean something only in that program, so on any
 * other — a board revision with a different EEPROM — nothing is written and
 * the part runs as it booted.
 *
 * Silent unless something plays. The DAC soft mute is held from probe and is
 * then ASoC's to move (.mute_stream): released when a stream is prepared,
 * asserted again when the last one closes — so nothing reaches the jacks at
 * power-on or while the output idles. On top of that the firmware boots the
 * program's Volume cell at 0, and audiod raises it once it has restored
 * the output's saved level and tone.
 *
 * 48 kHz only. That is the one rate the firmware lists, and the CE5300 I2S
 * this codec feeds from is fixed there too.
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
#include <linux/property.h>
#include <linux/regmap.h>
#include <sound/pcm.h>
#include <sound/soc.h>

#include "sigmadsp.h"

/*
 * ADAU145x core control registers, from the public ADAU1450/1451/1452
 * datasheet register map.
 */
#define ADAU1451_REG_HIBERNATE		0xf400
#define ADAU1451_REG_START_CORE		0xf402

#define ADAU1451_PARAM_MAXREG		0x1fff	/* vendor's ADAU1451_MAXREG */
#define ADAU1451_CTRL_MAXREG		0xf800

#define ADAU1451_HIBERNATE_DELAY_MS	255	/* vendor's number, not the datasheet's */
#define ADAU1451_RATE			48000

struct adau1451_c4 {
	struct regmap *param;		/* parameter RAM, 32-bit words */
	struct regmap *ctrl;		/* 0xf4xx core control, 16-bit words */
	struct sigmadsp *dsp;		/* NULL: run the program as it booted */
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
 * Verbatim from the vendor. HIBERNATE reads 1 after the hibernate above but 0
 * again once the core is running — that is what a stock unit shows mid-play,
 * and the vendor never writes it back — so starting the core is what lifts it.
 */
static int adau1451_c4_start(struct adau1451_c4 *st)
{
	int ret;

	ret = regmap_write(st->ctrl, ADAU1451_REG_START_CORE, 0) ?:
	      regmap_write(st->ctrl, ADAU1451_REG_START_CORE, 1);
	if (ret)
		return ret;

	usleep_range(1000, 2000);
	return 0;
}

/*
 * Bind: hand the firmware's controls to the card, then fill parameter RAM with
 * the core stopped and start it. The DAC stays muted throughout; only a stream
 * releases it (adau1451_c4_mute_stream).
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
 * Is the running program the one the firmware describes? The device tree
 * names a cell and the value it must hold. The read doubles as the liveness
 * check: a codec that is absent, or on another i2c bus than we think, NAKs
 * here at probe instead of turning into silence much later.
 */
static int adau1451_c4_program_ok(struct adau1451_c4 *st, struct device *dev)
{
	u32 want[2];
	unsigned int got;
	int ret;

	ret = regmap_read(st->param, 0, &got);
	if (ret)
		return dev_err_probe(dev, ret, "DSP did not respond\n");

	if (device_property_read_u32_array(dev, "control4,program-version",
					   want, ARRAY_SIZE(want))) {
		dev_warn(dev, "no control4,program-version: cannot tell which program is running, leaving it alone\n");
		return 0;
	}
	ret = regmap_read(st->param, want[0], &got);
	if (ret)
		return ret;
	if (got != want[1]) {
		dev_warn(dev, "program version cell 0x%04x reads %u, not %u: leaving the DSP as it booted\n",
			 want[0], got, want[1]);
		return 0;
	}
	return 1;
}

static int adau1451_c4_i2c_probe(struct i2c_client *client)
{
	struct device *dev = &client->dev;
	struct adau1451_c4 *st;
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

	ret = adau1451_c4_program_ok(st, dev);
	if (ret < 0)
		return ret;

	/*
	 * A missing or bad firmware file is not fatal: the DSP keeps playing
	 * with the parameters it booted with, there are just no DSP controls.
	 */
	if (ret && !device_property_read_string(dev, "firmware-name", &fw)) {
		st->dsp = devm_sigmadsp_init_i2c(client, NULL, fw);
		if (IS_ERR(st->dsp)) {
			dev_warn(dev, "no DSP parameters from %s (%ld): running the program as it booted\n",
				 fw, PTR_ERR(st->dsp));
			st->dsp = NULL;
		}
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
