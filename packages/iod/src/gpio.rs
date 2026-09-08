//! Just enough of the GPIO character device to pulse a reset line.
//!
//! Written against the **v2** ABI directly rather than through libgpiod,
//! because the two do not always both exist. This kernel has
//! `CONFIG_GPIO_CDEV=y` and `CONFIG_GPIO_CDEV_V1` unset, so every libgpiod-1.x
//! tool on the rootfs fails with `Invalid argument` — which is a poor reason to
//! have no way to recover a wedged microcontroller.
//!
//! Deliberately tiny. This is not a GPIO library; it drives one output line for
//! a moment, which is the entire requirement.
use std::ffi::CString;
use std::io;
use std::os::unix::io::RawFd;

const GPIO_MAX_NAME_SIZE: usize = 32;
const GPIO_V2_LINES_MAX: usize = 64;
const GPIO_V2_LINE_NUM_ATTRS_MAX: usize = 10;

const GPIO_V2_LINE_FLAG_OUTPUT: u64 = 1 << 1;

// Request numbers are 32-bit bit patterns, but the TYPE of ioctl's second
// argument is not portable: musl declares it `c_int`, glibc and macOS declare
// it `c_ulong`. Writing each literal as u32 and casting to the right alias
// keeps the bits identical either way.
//
// Worth spelling out rather than reaching for `libc::Ioctl`, which does not
// exist off Linux and so breaks `cargo check` on the machine these are written
// on. This is the class of bug the host build cannot catch at all — the targets
// disagree with each other AND with the host.
#[cfg(target_env = "musl")]
type Req = libc::c_int;
#[cfg(not(target_env = "musl"))]
type Req = libc::c_ulong;

// _IOR(0xB4, 0x01, struct gpiochip_info)
const GPIO_GET_CHIPINFO_IOCTL: Req = 0x8044_b401_u32 as Req;
// _IOWR(0xB4, 0x07, struct gpio_v2_line_request)
const GPIO_V2_GET_LINE_IOCTL: Req = 0xc250_b407_u32 as Req;
// _IOWR(0xB4, 0x0f, struct gpio_v2_line_values)
const GPIO_V2_LINE_SET_VALUES_IOCTL: Req = 0xc010_b40f_u32 as Req;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct LineAttribute {
    id: u32,
    padding: u32,
    value: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct LineConfigAttribute {
    attr: LineAttribute,
    mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LineConfig {
    flags: u64,
    num_attrs: u32,
    padding: [u32; 5],
    attrs: [LineConfigAttribute; GPIO_V2_LINE_NUM_ATTRS_MAX],
}

#[repr(C)]
struct LineRequest {
    offsets: [u32; GPIO_V2_LINES_MAX],
    consumer: [u8; GPIO_MAX_NAME_SIZE],
    config: LineConfig,
    num_lines: u32,
    event_buffer_size: u32,
    padding: [u32; 5],
    fd: i32,
}

#[repr(C)]
struct LineValues {
    bits: u64,
    mask: u64,
}

#[repr(C)]
struct ChipInfo {
    name: [u8; GPIO_MAX_NAME_SIZE],
    label: [u8; GPIO_MAX_NAME_SIZE],
    lines: u32,
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).to_string()
}

/// Find `/dev/gpiochipN` by its driver label.
///
/// board.env names the chip (`gpio_ich`) rather than a number because the
/// number is not stable: it depends on probe order, and the vendor's own notes
/// record the base moving between kernels.
pub fn find_chip(label: &str) -> io::Result<String> {
    for n in 0..16 {
        let name = format!("gpiochip{n}");
        let Ok(path) = CString::new(format!("/dev/{name}")) else { continue };
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            continue;
        }
        let mut info = ChipInfo { name: [0; GPIO_MAX_NAME_SIZE], label: [0; GPIO_MAX_NAME_SIZE], lines: 0 };
        let rc = unsafe { libc::ioctl(fd, GPIO_GET_CHIPINFO_IOCTL, &mut info) };
        unsafe { libc::close(fd) };
        if rc == 0 && cstr(&info.label) == label {
            return Ok(name);
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, format!("no gpiochip labelled {label}")))
}

/// A claimed output line.
///
/// **Held for the life of the daemon on purpose.** Releasing the fd hands the
/// pin back to the kernel, which returns it to its default — and for a reset
/// line whose default is not documented, that risks leaving the part held in
/// reset by the very call that was supposed to bring it back. Keeping the claim
/// also means a second reset reuses this line instead of failing EBUSY against
/// its own earlier request.
pub struct Line(RawFd);

impl Drop for Line {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Claim one line as an output and hold it, so the value survives until drop.
pub fn request_output(chip: &str, offset: u32, initial: bool) -> io::Result<Line> {
    let path = CString::new(format!("/dev/{chip}")).map_err(io::Error::other)?;
    let cfd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if cfd < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut req = LineRequest {
        offsets: [0; GPIO_V2_LINES_MAX],
        consumer: [0; GPIO_MAX_NAME_SIZE],
        config: LineConfig {
            flags: GPIO_V2_LINE_FLAG_OUTPUT,
            num_attrs: 0,
            padding: [0; 5],
            attrs: [LineConfigAttribute::default(); GPIO_V2_LINE_NUM_ATTRS_MAX],
        },
        num_lines: 1,
        event_buffer_size: 0,
        padding: [0; 5],
        fd: -1,
    };
    req.offsets[0] = offset;
    for (i, b) in b"iod".iter().enumerate() {
        req.consumer[i] = *b;
    }

    let rc = unsafe { libc::ioctl(cfd, GPIO_V2_GET_LINE_IOCTL, &mut req) };
    let err = io::Error::last_os_error();
    unsafe { libc::close(cfd) };
    if rc < 0 {
        return Err(err);
    }
    let line = Line(req.fd);
    set(&line, initial)?;
    Ok(line)
}

impl Line {
    pub fn set(&self, high: bool) -> io::Result<()> {
        set(self, high)
    }
}

fn set(line: &Line, high: bool) -> io::Result<()> {
    let v = LineValues { bits: if high { 1 } else { 0 }, mask: 1 };
    let rc = unsafe { libc::ioctl(line.0, GPIO_V2_LINE_SET_VALUES_IOCTL, &v) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
