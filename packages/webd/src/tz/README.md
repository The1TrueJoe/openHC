`zone.tab` and `backward` are copied unmodified from the IANA time zone database
(tzdata 2026e, https://www.iana.org/time-zones), which is in the public domain.

webd uses them only to place a time zone on the map: `zone.tab` gives each zone's
principal location, `backward` maps legacy names (`US/Eastern`, `Asia/Calcutta`,
what some browsers still report) to current ones. The display's Auto day/night
computes sunrise and sunset there (`src/daynight.rs`). Refresh both from a newer
tzdata release when zones change.
