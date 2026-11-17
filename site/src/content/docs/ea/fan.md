---
title: Fan and thermal
description: How the kernel runs the EA fan from the device tree — the 100 kHz PWM, the EA-3 v2's switched fan, the 30% floor, the trip curve, manual control and the 95 °C poweroff.
sidebar:
  order: 12
---

There is no fan daemon. The board device tree (`ea.dtsi`) describes the fan as a
mainline **`pwm-fan`** and puts it in a **thermal zone** that reads the LM75 on
the heatsink. The kernel's `step_wise` governor moves the fan along the curve.
sysmond only steps in for manual control and as a safety guard. The stock
interfaces this builds on are on the [EA-1 page](/ea/ea1/#fan-and-thermal--fully-open).

## The fan, and why the PWM runs at 100 kHz

The fan is a plain two-wire DC fan. The PWM (`pwm-ce5300` channel 2) switches
its **supply**; the fan has no PWM input of its own. So the PWM frequency decides
whether the fan's own driver chip gets a usable supply:

| PWM | What the fan does |
|---|---|
| ~206 Hz | Never starts. |
| 25 kHz (stock's 40 µs period) | Below ~70% duty the driver chip browns out, the motor stalls and chirps as it retries. |
| **100 kHz (10 µs)** | Runs clean down to 30%. Below that it grinds. |

Heard on an EA1, 2026-10-25 (PR #70). The tree uses
`pwms = <&pwm 2 10000 0>`.

**The 30% floor.** `cooling-levels = <0 77 102 128 153 179 204 230 255>`, which
is off, then 30% to 100% in tens. 77/255 is the lowest level that is not off,
and it is quiet (confirmed by ear, 2026-10-27). From a standstill, 25% already
started clean at 100 kHz, so the fan needs no start kick. If a different fan ever
does, that is pwm-fan's `fan-stop-to-start-percent`/`-us`.

## EA-3 v2: a switched fan with a speed pot

The EA-3 v2's fan isn't on the PWM. Its supply comes from a regulator switched
by `ldo_enable` (GPIO 43), and an AD5246 digital potentiometer at 0x2e on I2C
bus 0 sets its speed: a 7-bit wiper, power-on 0x40, higher is faster. Heard on
a v2 (straps 0x29), 2026-11-16:

- pwm-fan at 0%, 30% and 100% sounded the same.
- `ldo_enable` low stopped the fan, and high started it again.
- Wiper 0x00 was quieter and 0x7f louder.

This is stock `c4faultd`'s AD5246 path. On an EA it tries hwmon `pwm1`/`pwm2`, the
AD5246 (which also switches `ldo_enable`), PWM channel 2, then an LTC1695, and
uses the first one present. Stock's `c4_gpio_config` drives `ldo_enable` high on
every board, and nothing else in stock touches it.

So `ea3-v2.dts` replaces the pwm-fan with an **`io-channel-fan`** (the
iio-fan driver) under the same `fan` label, so `ea.dtsi`'s curve and cooling
maps apply unchanged:

- **Speed:** the AD5246, through the `mcp4018` IIO driver. A cooling level from
  0 to 255 is scaled onto the wiper's 0–127.
- **Power:** a `regulator-fixed` on GPIO 43 is the fan's `fan-supply`, so level 0
  turns the fan off. The `ldo_enable` hog is gone on the v2.
- **The floor:** 20% (wiper 25), then even steps to 100%. From a stop, 30%
  (wiper 38) and 20% (wiper 25) start the fan; 10% (wiper 13) and wiper 0 do
  not. Heard on ea3b, 2026-11-16.

The regulator core is only in the real kernel, so the bootstrap kernel drives
the speed but not the supply: GPIO 43 stays as CEFDK left it until `S00kexec`
hands over.

The AD5246's resistance variant (5, 10, 50 or 100 kΩ) can't be read over I2C,
since the part has no ID register, and this one hasn't been read off the
package. The tree says `adi,ad5246-010`. Only IIO's scale depends on it; the fan
uses raw wiper values.

The fan driver is its own repository, [iio-fan](https://github.com/The1TrueJoe/iio-fan),
a submodule at `board/ea/common/iio-fan` that the build copies into the kernel
tree. The AD5246 support in `mcp4018` is written for upstream (IIO); until it
lands, openHC carries it as patches 0025–0029.

sysmond finds the `iiofan` hwmon and `iio-fan` cooling device the same way as a
pwm-fan, and manual control works the same.

The EA-3 v1 has no AD5246. Its fan is assumed to be on PWM channel 2 like the
EA-1's, which isn't confirmed by ear yet.

## The curve

The zone `heatsink-thermal` polls the LM75 (`i2c-0` 0x48, hwmon label
`Heatsink`) every 5 s. Its trips are the curve, which is the stock `[ea_fan]`
profile (on at 50 °C, +10% per degree) with a 30% floor:

| Trip | Type | Effect |
|---|---|---|
| 50 °C | active | 30% |
| 51–56 °C | active | 40% … 90%, one level per degree |
| 57 °C | active | 100% |
| **80 °C** | hot | No kernel action. Ends a manual hold (see below). |
| **95 °C** | critical | Orderly poweroff. |

Each active trip has 1 °C of hysteresis, so a 0.5 °C LM75 step cannot make it
flap. On the first trip, that same band stands in for stock's cool-down hold:
the fan comes on at 50 °C and goes off below 49 °C.

How `step_wise` moves along that curve matters when you watch it. It steps
**up only while the temperature is rising**, and **down only while it is
falling**. At a flat temperature it holds where it is.

Until the zone binds, pwm-fan holds the fan at its top level (its probe sets the
maximum). So a sensor that never binds leaves the fan at 100%.

The trips are writable at runtime (`trip_point_N_temp`), which is the way to test
the curve without heating the box. The thermal core reprograms the LM75's
`temp1_max` to the next trip up, so that file no longer shows the chip's 80 °C
default.

## Manual control

The web UI's Health panel, REST and MQTT all go through sysmond:

```
REST  POST /api/fan/set {pct}   POST /api/fan/auto   GET /api/fan   (sysmond, 127.0.0.1:7071)
MQTT  openhc/<box>/cmd/health/fan     state: openhc/<box>/state/health/fan
```

- **Manual** switches the zone's `policy` to `user_space`, so the governor stops,
  then writes the fan's hwmon `pwm1`. A non-zero request below the floor is
  lifted to 30%.
- **Auto** writes the cooling device's `cur_state` to its maximum, then puts the
  policy back to `step_wise`. From 100% the fan walks down onto the curve as the
  box cools (full to off took about 110 s). Without that, the governor would keep
  the manual duty until the temperature next crossed a trip.

The mode **is** the zone's policy, not sysmond state. A restarted sysmond reads it
back, and a reboot comes up in auto.

A manual hold cannot cook the box. sysmond checks the zone on every 5 s sample:

- A manual set at or above the 80 °C hot trip is **refused**.
- A manual hold **ends** when the zone reaches the hot trip, and the fan goes
  back to the curve, which is at 100% by then.
- If the kernel disables the zone because its sensor stopped answering, sysmond
  puts the fan at **100%**.

The Health panel reads the heatsink temperature **from the zone**. sysmond
matches the zone's `thermal-sensors` phandle to the LM75's hwmon `of_node` and
hides that raw channel, so the board shows one temperature, not two. The
fan's duty comes from pwm-fan's hwmon. The names "Heatsink" and "Fan" are the
device tree `label`s.

## The 95 °C critical trip

At 95 °C the thermal core does an orderly hw_protection poweroff. That is 38 °C
past the fan's 100% point, so it only fires when cooling has failed: a dead fan,
or a blocked vent.

95 °C sits between a 90 °C backstop and the die's own limit, Intel's TjMax of
100 °C. That TjMax is the closest thing to a manufacturer figure:

- The CE5310 has no `MSR_IA32_TEMPERATURE_TARGET` (0x1a2). Reading it returns
  EIO. Intel publishes no TjMax for any CE53xx part.
- The core is **Saltwell**, family 6 model 0x36, the same as Cedarview. The
  kernel's `coretemp` takes 100 °C as TjMax for that model.
- Control4 rates the EA-1 and EA-3 only for 0–40 °C **ambient**.

The LM75 is on the heatsink, and the heatsink always reads below the die. On an
EA1 on 2026-10-27, the heatsink read 43 °C while the die's digital readout
(`IA32_THERM_STATUS`, 0x88160000) was 22 °C under TjMax. So by the time the
heatsink reads 95 °C, the die is already past its limit, and the CPU's own
throttling and THERMTRIP# are at work. The trip is a backstop for the board,
not a tuned limit.

## What is verified

On an EA1 (ramboot of PR #83's build, 2026-10-27):

- pwm-fan and the zone bind with no daemon running, and the curve steps as
  above (tested with the trips lowered at runtime).
- Manual control over REST and MQTT, the floor, the refusal above the hot trip,
  the hold ending at the hot trip (tested with the hot trip lowered to 35 °C),
  and the UI's release to automatic.
- The Health panel's heatsink temperature from the zone and fan duty from
  pwm-fan.

Not verified on hardware:

- The "Heatsink" and "Fan" names in the Health panel. They come from labels that
  landed after that test build.
- The fail-safe to 100% when the zone is disabled because the sensor is lost.
- The 95 °C poweroff. A heatsink at 95 °C can't be reached safely.
- **Any of this on an EA-3.** Its fan does not spin under openHC at all: the PWM
  waveform is right, but a global enable that stock's proprietary driver set at
  load is missing.
