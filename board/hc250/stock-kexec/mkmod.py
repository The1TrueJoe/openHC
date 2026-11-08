# modpost stand-in: ohc_kexec.mod.c from the object's undefined symbols and
# the CRCs the stock modules carry (stock-crcs.txt: "0xCRC<TAB>name").
import os, subprocess, sys
obj, crcfile, out = sys.argv[1:4]
crcs = {}
for l in open(crcfile):
    if not l.strip() or l.startswith('#'):
        continue
    c, n = l.split()
    crcs[n] = c
und = [l.split()[-1] for l in subprocess.check_output([os.environ.get('NM', 'arm-none-eabi-nm'), '-u', obj], text=True).splitlines()]
und = [u for u in und if u != '__this_module']
miss = [u for u in und if u not in crcs]
if miss:
    sys.exit('no stock CRC for: ' + ' '.join(miss))
v = '\n'.join(f'\t{{ {crcs[n]}, "{n}" }},' for n in ['module_layout'] + und)
open(out, 'w').write(f'''#include <linux/module.h>
#include <linux/vermagic.h>
#include <linux/compiler.h>

MODULE_INFO(vermagic, VERMAGIC_STRING);

struct module __this_module
__attribute__((section(".gnu.linkonce.this_module"))) = {{
	.name = KBUILD_MODNAME,
	.init = init_module,
	.exit = cleanup_module,
	.arch = MODULE_ARCH_INIT,
}};

static const struct modversion_info ____versions[]
__used __attribute__((section("__versions"))) = {{
{v}
}};

static const char __module_depends[]
__used __attribute__((section(".modinfo"))) = "depends=";
''')
