# ADAU1451 SigmaDSP (EA3 / EA5)

The EA3 and EA5 route audio through an Analog Devices **ADAU1451** SigmaDSP.
Confirmed on a live EA3: the part sits on **i2c-3 at 0x38**, driven by
`snd_soc_adau1451` + `snd_soc_adau1451_common` + `snd_soc_sigmadsp_{i2c,new}`,
with reset on `/dev/gpio/dsp_reset` (gpio101).

```
adau1451 3-0038: sigmadsp_firmware_load: ### Load Firmware v2
ninjago-smd-codec.1: adau1451-ch0 <-> ninjago-smd-dai.0 mapping ok
```

It presents as its own ALSA card:

```
card 1 [ninjagosmdadau1]  "ninjago-smd-adau1451"
  device 0  SMD DAI PCM adau1451-ch0-0
controls  Master Bass Treble Balance Loudness VolumeCurve InputGain Input
          OutputMode OutputParams FirmwareRate
          Filter31_5 63 125 250 500 1000 2000 4000 8000 16000
```

That control set — a 10-band graphic EQ plus tone and loudness — is the DSP
program, not a generic codec. The SoC side (`card 0 IntelCE353xx`) carries
`analog0` / `digital0` / `hdmi0`, and the SoC↔DSP link is I²S1
(`i2s_audio_pref enabled=2` in `/etc/hdmi_hpd.cfg`).

## Firmware blobs

Per-board SigmaStudio program images live on the stock rootfs:

```
/lib/firmware/ea3-1451.bin    80,040 B
/lib/firmware/ea5-1451.bin   152,392 B
/lib/firmware/tr1-1451.bin    99,560 B
loaders: /control4/bin/{ea3,ea5,tr1}_dsploader
```

**These are vendor firmware and are not redistributed here** — pull them off
your own unit. Load tooling would live in this directory.

Not used on the EA1.

## Open question

The **AK4621EF** analog codec that sits between the DSP and the line/coax jacks
has no kernel driver and no I²C client. Its control port is probably one of the
two userspace-exposed `spidev` nodes (`/dev/spidev0.2`, `/dev/spidev0.3`) with
`codec_reset` (gpio24) as reset — but nothing on the host names it, so which
node, or whether it is strapped into a standalone mode instead, is unresolved.
This is the biggest gap in the EA3 audio path.
