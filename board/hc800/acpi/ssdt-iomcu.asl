/*
 * openHC — the HC-800's IO microcontroller, described to the kernel.
 *
 * The LM3S1162 hangs off the 16550A at 0x2E8 (ttyS3). The BIOS describes that
 * UART (a PNP0501 under the LPC bridge) and nothing behind it, so this table
 * adds the missing child. serdev finds it by its UartSerialBusV2 resource, and
 * gpio-ohc-iomcu binds it at boot exactly as it binds the EA's device tree
 * node: _HID PRP0001 plus a "compatible" in _DSD make the driver's OF match
 * table and device_property_*() reads work unchanged, so the binding below is
 * the same one the EA .dts files use.
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
             * Every count here was settled on a live unit
             * (https://the1truejoe.github.io/openHC/hc800/#io-an-lm3s1162-on-ttys3):
             *
             *   115200   CONFIRMED on hardware, unlike the EA's 460800.
             *   IR       SEVEN emitters: six rear jacks plus the internal front
             *            blaster. Sweeping the IROUT_SEND output bitmask with a
             *            Global Cache learner watching: bit 0 lit jack 1, bit 5
             *            jack 6, bit 6 the front of the box, nothing else in the
             *            24 bits did anything. Bits 5 and 6 together fired both,
             *            which is what proves the field is a mask, not an index.
             *            Plus the front receiver, for learning.
             *   relays   FOUR, all working. They once looked like two: the
             *            selector is a 0-BASED INDEX and was being sent as a
             *            bitmask, and the firmware echoes 0x04/0x08 happily
             *            while addressing nothing. A TOGGLE sweep settled it:
             *            0x00..0x03 each flip one relay, 0x04 up do nothing.
             *   contacts FOUR.
             *   uarts    NONE MCU-routed. The rear RS-232 ports are host
             *            16550As (ttyS1/ttyS2): the vendor's ioserver holds
             *            them directly, alongside this one.
             */
            Name (_DSD, Package ()
            {
                ToUUID ("daffd814-6eba-4d8c-8a91-bc9bbf4aa301"),
                Package ()
                {
                    Package () { "compatible", "control4,io-mcu" },
                    Package () { "current-speed", 115200 },
                    Package () { "control4,relays", 4 },
                    Package () { "control4,contacts", 4 },
                    Package () { "control4,ir-outputs", 6 },
                    Package () { "control4,ir-blaster", 1 },
                    Package () { "control4,ir-receiver", 1 },
                }
            })
        }
    }
}
