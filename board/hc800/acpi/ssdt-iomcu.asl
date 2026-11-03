/*
 * openHC — the HC-800's IO microcontroller, described to the kernel.
 *
 * The LM3S1162 hangs off the 16550A at 0x2E8 (ttyS3). The BIOS describes that
 * UART (a PNP0501 under the LPC bridge) and nothing behind it, so this table
 * adds the missing child. serdev finds it by its UartSerialBusV2 resource, and
 * gpio-ohc-iomcu binds it at boot exactly as it binds the EA's device tree
 * node: _HID PRP0001 plus a "compatible" in _DSD make the driver's OF match
 * table and fwnode reads work unchanged, and the hierarchical data nodes below
 * are the binding's child nodes (control4,io-mcu.yaml).
 *
 * ACPI, not the board device tree, on purpose: on an x86 box whose interrupts
 * belong to ACPI a device-tree UART has no IRQ and is timer-polled, and a
 * polled 16550A at 115200 overruns its 16-byte FIFO on every IR capture. Here
 * the UART stays the BIOS's, IRQ and all, and only the child is ours.
 *
 * Loaded as an ACPI table upgrade: post-image.sh prepends it to the initrd as
 * kernel/firmware/acpi/ssdt-iomcu.aml (CONFIG_ACPI_TABLE_UPGRADE). Nothing on
 * the box's firmware or disk changes.
 */
/*
 * The path is the BIOS's own, read off a live unit
 * (/sys/bus/pnp/devices/00:03/firmware_node/path): all five UARTs are PNP0501
 * under \_SB.PCI0.SBRG, and UAR4 is the one at 0x2E8 on IRQ 10.
 */
DefinitionBlock ("", "SSDT", 2, "OPENHC", "IOMCU", 0x00000001)
{
    External (\_SB.PCI0.SBRG.UAR4, DeviceObj)

    Scope (\_SB.PCI0.SBRG.UAR4)
    {
        Device (IOMC)
        {
            Name (_HID, "PRP0001")
            Name (_DDN, "Control4 IO microcontroller (LM3S1162)")
            Name (_CRS, ResourceTemplate ()
            {
                UartSerialBusV2 (115200, DataBitsEight, StopBitsOne,
                    0x00, LittleEndian, ParityTypeNone, FlowControlNone,
                    0x0010, 0x0010, "\\_SB.PCI0.SBRG.UAR4",
                    0x00, ResourceConsumer, , Exclusive, )
            })
            /*
             * Settled on a live unit (docs: hc800, IO): 115200; six rear IR
             * jacks plus the front blaster on IROUT_SEND bit 6; the front
             * receiver; four relays (a 0-based index on the wire) and four
             * contacts; no MCU-routed serial (the rear RS-232 ports are host
             * UARTs). Pins (control4,ir-pin, control4,pin) go in once the
             * LM3S1162 pin map is decoded; until then the driver sends no
             * pin table and the part runs its own.
             */
            Name (_DSD, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "compatible", Package () {
                        "control4,lm3s1162-io-mcu", "control4,io-mcu" } },
                    Package () { "current-speed", 115200 },
                    /* an empty ir-receiver node would not exist in ACPI */
                    Package () { "control4,ir-receiver", 1 },
                },
                ToUUID ("dbb8e3e6-5886-4ba6-8795-1319f52a966b"),
                Package ()
                {
                    Package () { "jack@1", "JK01" },
                    Package () { "jack@2", "JK02" },
                    Package () { "jack@3", "JK03" },
                    Package () { "jack@4", "JK04" },
                    Package () { "jack@5", "JK05" },
                    Package () { "jack@6", "JK06" },
                    Package () { "ir-blaster", "IRBL" },
                    Package () { "relays", "RLYS" },
                    Package () { "contacts", "CONS" },
                }
            })
            Name (JK01, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "reg", 1 },
                    Package () { "label", "IR 1" },
                    Package () { "control4,ir-channel", 0 },
                }
            })
            Name (JK02, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "reg", 2 },
                    Package () { "label", "IR 2" },
                    Package () { "control4,ir-channel", 1 },
                }
            })
            Name (JK03, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "reg", 3 },
                    Package () { "label", "IR 3" },
                    Package () { "control4,ir-channel", 2 },
                }
            })
            Name (JK04, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "reg", 4 },
                    Package () { "label", "IR 4" },
                    Package () { "control4,ir-channel", 3 },
                }
            })
            Name (JK05, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "reg", 5 },
                    Package () { "label", "IR 5" },
                    Package () { "control4,ir-channel", 4 },
                }
            })
            Name (JK06, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "reg", 6 },
                    Package () { "label", "IR 6" },
                    Package () { "control4,ir-channel", 5 },
                }
            })
            Name (IRBL, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "control4,ir-channel", 6 } }
            })
            Name (RLYS, Package ()
            {
                ToUUID ("dbb8e3e6-5886-4ba6-8795-1319f52a966b"),
                Package ()
                {
                    Package () { "relay@0", "RLY0" },
                    Package () { "relay@1", "RLY1" },
                    Package () { "relay@2", "RLY2" },
                    Package () { "relay@3", "RLY3" },
                }
            })
            Name (RLY0, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 0 } }
            })
            Name (RLY1, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 1 } }
            })
            Name (RLY2, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 2 } }
            })
            Name (RLY3, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 3 } }
            })
            Name (CONS, Package ()
            {
                ToUUID ("dbb8e3e6-5886-4ba6-8795-1319f52a966b"),
                Package ()
                {
                    Package () { "contact@0", "CON0" },
                    Package () { "contact@1", "CON1" },
                    Package () { "contact@2", "CON2" },
                    Package () { "contact@3", "CON3" },
                }
            })
            Name (CON0, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 0 } }
            })
            Name (CON1, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 1 } }
            })
            Name (CON2, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 2 } }
            })
            Name (CON3, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package () { Package () { "reg", 3 } }
            })
        }
    }
}
