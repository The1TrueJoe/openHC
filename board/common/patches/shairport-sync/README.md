# shairport-sync patches (Buildroot `BR2_GLOBAL_PATCH_DIR`)

Applied to Buildroot's own shairport-sync on every board with the `audio`
feature (the EA family and the HC-800 list this directory in their
`BR2_GLOBAL_PATCH_DIR`). audiod runs several AirPlay 2 receivers on one host and
drives each over D-Bus, which needs two things upstream (5.6.2) does not do:

* `0001` — a second instance on the bus takes `org.gnome.ShairportSync.i<pid>`
  instead of running with no D-Bus name at all.
* `0002` — setting `Volume` over D-Bus sets the volume the next session starts
  at, not only the current session's.
