/*
 * Safe Rust Implementation: V4L2 Decoder -> DMA-BUF fd -> DRM Atomic Commit -> HDMI Pipeline
 * Target: Radxa Rock 5C+ / Rockchip RK3588 running Armbian
 */

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::ptr;
use std::thread;
use std::time::Duration;
use std::num::NonZeroU32;
use core::ffi::c_void;

use drm::control::{
    connector, framebuffer,
    Device as ControlDevice, Mode,
};
use drm::Device as DrmDevice;
use libc::{
    c_int, c_ulong, MAP_FAILED, MAP_SHARED, O_CLOEXEC, O_NONBLOCK, O_RDWR,
    PROT_READ, PROT_WRITE,
};

// DRM Card wrapper using official drm crate
#[derive(Debug)]
struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl DrmDevice for Card {}
impl ControlDevice for Card {}

// FourCC Pixel Formats
const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const DRM_FORMAT_NV12: u32 = u32::from_le_bytes(*b"NV12");

// V4L2 Constants
const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;
const V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE: u32 = 10;
const V4L2_MEMORY_MMAP: u32 = 1;

const V4L2_CAP_VIDEO_CAPTURE_MPLANE: u32 = 0x00001000;
const V4L2_CAP_VIDEO_M2M: u32 = 0x00004000;
const V4L2_CAP_VIDEO_M2M_MPLANE: u32 = 0x00008000;

const V4L2_PIX_FMT_NV12: u32 = u32::from_le_bytes(*b"NV12");
const V4L2_PIX_FMT_BGR32: u32 = u32::from_le_bytes(*b"AR24");

const VIDIOC_QUERYCAP: c_ulong = 0x80685600;
const VIDIOC_S_FMT: c_ulong = 0xc0cc5605;
const VIDIOC_REQBUFS: c_ulong = 0xc0145608;
const VIDIOC_QUERYBUF: c_ulong = 0xc0445609;
const VIDIOC_EXPBUF: c_ulong = 0xc0405654;

const DRM_IOCTL_MODE_CREATE_DUMB: c_ulong = 0xc02064b2;
const DRM_IOCTL_MODE_MAP_DUMB: c_ulong = 0xc01064b3;
const DRM_IOCTL_PRIME_HANDLE_TO_FD: c_ulong = 0xc00c642d;

// V4L2 Struct Definitions
#[repr(C)]
struct V4l2Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    enc_or_ycbcr: u32,
    quantization: u32,
    xfer_func: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PlaneFormat {
    sizeimage: u32,
    bytesperline: u32,
    reserved: [u16; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PixFormatMplane {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    colorspace: u32,
    plane_fmt: [V4l2PlaneFormat; 8],
    num_planes: u8,
    flags: u8,
    enc_or_ycbcr: u8,
    quantization: u8,
    xfer_func: u8,
    reserved: [u8; 7],
}

#[repr(C)]
struct V4l2Format {
    type_: u32,
    fmt: V4l2FormatUnion,
}

#[repr(C)]
union V4l2FormatUnion {
    pix: V4l2PixFormat,
    pix_mp: V4l2PixFormatMplane,
    raw_data: [u8; 200],
}

#[repr(C)]
struct V4l2RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    capabilities: u32,
    reserved: [u32; 1],
}

#[repr(C)]
struct V4l2Plane {
    bytesused: u32,
    length: u32,
    m: V4l2PlaneUnion,
    data_offset: u32,
    reserved: [u32; 11],
}

#[repr(C)]
union V4l2PlaneUnion {
    mem_offset: u32,
    userptr: c_ulong,
    fd: c_int,
}

#[repr(C)]
struct V4l2Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: [i64; 2],
    timecode: [u32; 4],
    sequence: u32,
    memory: u32,
    m: V4l2BufferUnion,
    length: u32,
    reserved2: u32,
    reserved: u32,
}

#[repr(C)]
union V4l2BufferUnion {
    offset: u32,
    userptr: c_ulong,
    planes: *mut V4l2Plane,
    fd: c_int,
}

#[repr(C)]
struct V4l2ExportBuffer {
    type_: u32,
    index: u32,
    plane: u32,
    flags: u32,
    fd: c_int,
    reserved: [u32; 11],
}

#[repr(C)]
struct DrmModeCreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
struct DrmModeMapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
struct DrmPrimeHandle {
    handle: u32,
    flags: u32,
    fd: c_int,
}

#[link(name = "drm")]
extern "C" {
    fn drmPrimeFDToHandle(fd: c_int, prime_fd: u32, handle: *mut u32) -> c_int;
    fn drmModeAddFB2(
        fd: c_int, width: u32, height: u32, pixel_format: u32,
        bo_handles: *const u32, pitches: *const u32, offsets: *const u32,
        buf_id: *mut u32, flags: u32,
    ) -> c_int;
    fn drmModeRmFB(fd: c_int, buffer_id: u32) -> c_int;
}

// 100% Safe Rust: Draw ARGB8888 rectangles
fn draw_rectangles_argb(buf: &mut [u32], width: u32, height: u32) {
    let bg_color = 0xFF1E1E24;
    buf.fill(bg_color);

    let rect_x = (width / 4) as usize;
    let rect_y = (height / 4) as usize;
    let rect_w = (width / 2) as usize;
    let rect_h = (height / 2) as usize;
    let mut border_thick = (width / 160) as usize;
    if border_thick < 4 { border_thick = 4; }

    let border_color = 0xFF00FFCC; // Cyan
    let fill_color   = 0xFFFF3366; // Coral Pink

    let inner_x = rect_x + border_thick;
    let inner_y = rect_y + border_thick;
    let inner_w = rect_w.saturating_sub(2 * border_thick);
    let inner_h = rect_h.saturating_sub(2 * border_thick);

    for y in rect_y..(rect_y + rect_h) {
        for x in rect_x..(rect_x + rect_w) {
            if y < height as usize && x < width as usize {
                let idx = y * (width as usize) + x;
                if idx < buf.len() {
                    if x >= inner_x && x < inner_x + inner_w && y >= inner_y && y < inner_y + inner_h {
                        buf[idx] = fill_color;
                    } else {
                        buf[idx] = border_color;
                    }
                }
            }
        }
    }

    let box2_w = rect_w / 3;
    let box2_h = rect_h / 3;
    let box2_x = rect_x + (rect_w - box2_w) / 2;
    let box2_y = rect_y + (rect_h - box2_h) / 2;
    let box2_color = 0xFFFFCC00; // Bright yellow

    for y in box2_y..(box2_y + box2_h) {
        for x in box2_x..(box2_x + box2_w) {
            if y < height as usize && x < width as usize {
                let idx = y * (width as usize) + x;
                if idx < buf.len() {
                    buf[idx] = box2_color;
                }
            }
        }
    }
}

// 100% Safe Rust: Draw NV12 YUV rectangles
fn draw_rectangles_nv12(buf: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;

    if buf.len() < y_size + y_size / 2 { return; }

    let (y_plane, uv_plane) = buf.split_at_mut(y_size);

    y_plane.fill(40);
    uv_plane.fill(128);

    let rect_x = w / 4;
    let rect_y = h / 4;
    let rect_w = w / 2;
    let rect_h = h / 2;
    let mut border_thick = w / 160;
    if border_thick < 4 { border_thick = 4; }

    for r in rect_y..(rect_y + rect_h) {
        for c in rect_x..(rect_x + rect_w) {
            let is_border = r < rect_y + border_thick || r >= rect_y + rect_h - border_thick ||
                            c < rect_x + border_thick || c >= rect_x + rect_w - border_thick;

            if r < h && c < w {
                y_plane[r * w + c] = if is_border { 170 } else { 145 };

                if r % 2 == 0 && c % 2 == 0 {
                    let uv_idx = (r / 2) * w + (c & !1);
                    if uv_idx + 1 < uv_plane.len() {
                        uv_plane[uv_idx]     = if is_border { 166 } else { 54 };
                        uv_plane[uv_idx + 1] = if is_border { 16 }  else { 34 };
                    }
                }
            }
        }
    }
}

fn create_drm_dmabuf_fallback(drm_fd: RawFd, width: u32, height: u32) -> Result<(i32, u32, u32, usize, *mut c_void), String> {
    unsafe {
        let mut create_dumb = DrmModeCreateDumb {
            width,
            height,
            bpp: 32,
            flags: 0,
            handle: 0,
            pitch: 0,
            size: 0,
        };

        if libc::ioctl(drm_fd, DRM_IOCTL_MODE_CREATE_DUMB, &mut create_dumb) < 0 {
            return Err("DRM_IOCTL_MODE_CREATE_DUMB failed".to_string());
        }

        let mut prime = DrmPrimeHandle {
            handle: create_dumb.handle,
            flags: (O_CLOEXEC | O_RDWR) as u32,
            fd: -1,
        };

        if libc::ioctl(drm_fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &mut prime) < 0 {
            return Err("DRM_IOCTL_PRIME_HANDLE_TO_FD failed".to_string());
        }

        let mut map_dumb = DrmModeMapDumb {
            handle: create_dumb.handle,
            pad: 0,
            offset: 0,
        };

        if libc::ioctl(drm_fd, DRM_IOCTL_MODE_MAP_DUMB, &mut map_dumb) < 0 {
            return Err("DRM_IOCTL_MODE_MAP_DUMB failed".to_string());
        }

        let ptr = libc::mmap(
            ptr::null_mut(),
            create_dumb.size as usize,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            drm_fd,
            map_dumb.offset as libc::off_t,
        );

        if ptr == MAP_FAILED {
            return Err("mmap failed".to_string());
        }

        Ok((prime.fd, create_dumb.handle, create_dumb.pitch, create_dumb.size as usize, ptr))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=====================================================");
    println!(" Safe Rust Pipeline: V4L2 -> DMA-BUF fd -> DRM Atomic Commit -> HDMI");
    println!(" Radxa Rock 5C+ / Rockchip RK3588 DRM Display");
    println!(" Dynamic Resolution Autodetection");
    println!("=====================================================\n");

    let args: Vec<String> = std::env::args().collect();
    let requested_dev = args.get(1).map(|s| s.as_str()).unwrap_or("/dev/video0");

    // -------------------------------------------------------------
    // Step 1: Open DRM Card Device using official `drm` crate
    // -------------------------------------------------------------
    println!("[STEP 1] Opening DRM device via safe `drm` crate...");
    let mut card_file = None;

    for i in 0..4 {
        let card_path = format!("/dev/dri/card{}", i);
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(&card_path) {
            let card = Card(file);
            // Check if card has connectors & CRTCs
            if let Ok(handles) = card.resource_handles() {
                if handles.connectors().len() > 0 && handles.crtcs().len() > 0 {
                    println!("[DRM SUCCESS] Opened display card: {}", card_path);
                    card_file = Some(card);
                    break;
                }
            }
        }
    }

    let card = card_file.ok_or_else(|| "Could not find an active DRM display card")?;

    // Enable client caps safely
    card.set_client_capability(drm::ClientCapability::UniversalPlanes, true)?;
    card.set_client_capability(drm::ClientCapability::Atomic, true)?;

    let resources = card.resource_handles()?;
    let mut target_connector = None;
    let mut selected_mode: Option<Mode> = None;

    // Find connected HDMI connector and preferred mode
    for &conn_handle in resources.connectors() {
        if let Ok(conn_info) = card.get_connector(conn_handle, true) {
            if conn_info.state() == connector::State::Connected {
                let conn_type = conn_info.interface();
                if conn_type == connector::Interface::HDMIA || conn_type == connector::Interface::HDMIB {
                    println!("[DRM] Found connected HDMI connector: {:?}", conn_handle);
                    for mode in conn_info.modes() {
                        if mode.mode_type().contains(drm::control::ModeTypeFlags::PREFERRED) {
                            println!("[DRM] Found PREFERRED mode: {}x{} @ {}Hz", mode.size().0, mode.size().1, mode.vrefresh());
                            selected_mode = Some(*mode);
                            break;
                        }
                    }
                    if selected_mode.is_none() && !conn_info.modes().is_empty() {
                        selected_mode = Some(conn_info.modes()[0]);
                    }
                    target_connector = Some(conn_handle);
                    break;
                }
            }
        }
    }

    let conn_handle = target_connector.ok_or_else(|| "No connected HDMI connector found")?;
    let mode = selected_mode.ok_or_else(|| "No mode found for connected HDMI display")?;

    let (screen_w, screen_h) = (mode.size().0 as u32, mode.size().1 as u32);
    println!("[DRM AUTODETECT SUCCESS] Screen Resolution: {}x{} @ {}Hz (Connector: {:?})",
             screen_w, screen_h, mode.vrefresh(), conn_handle);

    let crtc_handle = resources.crtcs()[0];
    println!("[DRM] Selected CRTC: {:?}", crtc_handle);

    // -------------------------------------------------------------
    // Step 2: V4L2 Buffer Allocation with Autodetected Resolution
    // -------------------------------------------------------------
    println!("\n[STEP 2] Opening V4L2 device and setting target {}x{} resolution...", screen_w, screen_h);
    let best_v4l2_dev = requested_dev.to_string();
    println!("[V4L2] Target device node: {}", best_v4l2_dev);

    let drm_raw_fd = card.as_fd().as_raw_fd();
    let mut fb_w = screen_w;
    let mut fb_h = screen_h;
    let mut dmabuf_fd: RawFd = -1;
    let mut buf_map: *mut c_void = ptr::null_mut();
    let mut buf_size: usize = 0;
    let mut pixel_format = DRM_FORMAT_NV12;
    let mut pitch = fb_w;
    let mut v4l2_success = false;

    if let Ok(c_v4l2_path) = CString::new(best_v4l2_dev.clone()) {
        let v_fd = unsafe { libc::open(c_v4l2_path.as_ptr(), O_RDWR | O_NONBLOCK) };

        if v_fd >= 0 {
            unsafe {
                let mut cap: V4l2Capability = std::mem::zeroed();
                if libc::ioctl(v_fd, VIDIOC_QUERYCAP, &mut cap) == 0 {
                    let driver_str = std::str::from_utf8(&cap.driver).unwrap_or("").trim_matches('\0');
                    let card_str = std::str::from_utf8(&cap.card).unwrap_or("").trim_matches('\0');
                    println!("[V4L2] Driver: {}, Card: {}", driver_str, card_str);

                    let buf_type = if (cap.capabilities & V4L2_CAP_VIDEO_CAPTURE_MPLANE) != 0 {
                        V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
                    } else {
                        V4L2_BUF_TYPE_VIDEO_CAPTURE
                    };

                    if (cap.capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE)) != 0 {
                        let mut fmt_out: V4l2Format = std::mem::zeroed();
                        fmt_out.type_ = if (cap.capabilities & V4L2_CAP_VIDEO_M2M_MPLANE) != 0 {
                            V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE
                        } else {
                            V4L2_BUF_TYPE_VIDEO_OUTPUT
                        };
                        fmt_out.fmt.pix.width = screen_w;
                        fmt_out.fmt.pix.height = screen_h;
                        fmt_out.fmt.pix.pixelformat = V4L2_PIX_FMT_NV12;
                        libc::ioctl(v_fd, VIDIOC_S_FMT, &mut fmt_out);
                    }

                    let mut fmt: V4l2Format = std::mem::zeroed();
                    fmt.type_ = buf_type;
                    if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE {
                        fmt.fmt.pix_mp.width = screen_w;
                        fmt.fmt.pix_mp.height = screen_h;
                        fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_NV12;
                        fmt.fmt.pix_mp.num_planes = 1;
                    } else {
                        fmt.fmt.pix.width = screen_w;
                        fmt.fmt.pix.height = screen_h;
                        fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_NV12;
                    }

                    if libc::ioctl(v_fd, VIDIOC_S_FMT, &mut fmt) < 0 {
                        if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE {
                            fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_BGR32;
                        } else {
                            fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_BGR32;
                        }
                        libc::ioctl(v_fd, VIDIOC_S_FMT, &mut fmt);
                    }

                    let (negotiated_w, negotiated_h, negotiated_fmt) = if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE {
                        (fmt.fmt.pix_mp.width, fmt.fmt.pix_mp.height, fmt.fmt.pix_mp.pixelformat)
                    } else {
                        (fmt.fmt.pix.width, fmt.fmt.pix.height, fmt.fmt.pix.pixelformat)
                    };

                    if negotiated_w > 0 && negotiated_h > 0 {
                        fb_w = negotiated_w;
                        fb_h = negotiated_h;
                    }

                    if negotiated_fmt == V4L2_PIX_FMT_BGR32 {
                        pixel_format = DRM_FORMAT_XRGB8888;
                        pitch = fb_w * 4;
                    } else {
                        pixel_format = DRM_FORMAT_NV12;
                        pitch = fb_w;
                    }
                    println!("[V4L2] Negotiated format: {} ({}x{}), pitch: {}",
                             if pixel_format == DRM_FORMAT_NV12 { "NV12" } else { "XRGB8888" }, fb_w, fb_h, pitch);

                    let mut reqbuf: V4l2RequestBuffers = std::mem::zeroed();
                    reqbuf.count = 1;
                    reqbuf.type_ = buf_type;
                    reqbuf.memory = V4L2_MEMORY_MMAP;

                    if libc::ioctl(v_fd, VIDIOC_REQBUFS, &mut reqbuf) == 0 && reqbuf.count > 0 {
                        let mut buf: V4l2Buffer = std::mem::zeroed();
                        let mut planes: [V4l2Plane; 1] = std::mem::zeroed();
                        buf.type_ = buf_type;
                        buf.memory = V4L2_MEMORY_MMAP;
                        buf.index = 0;

                        if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE {
                            buf.m.planes = planes.as_mut_ptr();
                            buf.length = 1;
                        }

                        if libc::ioctl(v_fd, VIDIOC_QUERYBUF, &mut buf) == 0 {
                            let offset = if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE { planes[0].m.mem_offset as usize } else { buf.m.offset as usize };
                            buf_size = if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE { planes[0].length as usize } else { buf.length as usize };

                            let mapped = libc::mmap(
                                ptr::null_mut(),
                                buf_size,
                                PROT_READ | PROT_WRITE,
                                MAP_SHARED,
                                v_fd,
                                offset as libc::off_t,
                            );

                            if mapped != MAP_FAILED {
                                buf_map = mapped;

                                let mut expbuf: V4l2ExportBuffer = std::mem::zeroed();
                                expbuf.type_ = buf_type;
                                expbuf.index = 0;
                                expbuf.flags = (O_CLOEXEC | O_RDWR) as u32;

                                if libc::ioctl(v_fd, VIDIOC_EXPBUF, &mut expbuf) == 0 {
                                    dmabuf_fd = expbuf.fd;
                                    v4l2_success = true;
                                    println!("[V4L2 SUCCESS] Exported DMA-BUF fd = {}, size = {} bytes", dmabuf_fd, buf_size);

                                    if pixel_format == DRM_FORMAT_NV12 || buf_size < (fb_w * fb_h * 4) as usize {
                                        let slice = std::slice::from_raw_parts_mut(buf_map as *mut u8, buf_size);
                                        draw_rectangles_nv12(slice, fb_w, fb_h);
                                    } else {
                                        let slice = std::slice::from_raw_parts_mut(buf_map as *mut u32, buf_size / 4);
                                        draw_rectangles_argb(slice, fb_w, fb_h);
                                    }
                                    println!("[V4L2] Drawn rectangle on V4L2 DMA-BUF frame memory ({}x{}).", fb_w, fb_h);
                                }
                            }
                        }
                    }
                }
                libc::close(v_fd);
            }
        }
    }

    // Allocate native 1:1 screen resolution DMA-BUF buffer if V4L2 hardware clamped size
    if !v4l2_success || dmabuf_fd < 0 || fb_w < screen_w {
        if v4l2_success && fb_w < screen_w {
            println!("[INFO] V4L2 hardware node clamped buffer to {}x{}; allocating native 1:1 screen resolution ({}x{}) DMA-BUF...",
                     fb_w, fb_h, screen_w, screen_h);
            unsafe {
                if !buf_map.is_null() && buf_size > 0 {
                    libc::munmap(buf_map, buf_size);
                }
                if dmabuf_fd >= 0 { libc::close(dmabuf_fd); }
            }
        }

        println!("[INFO] Allocating native DRM PRIME DMA-BUF buffer ({}x{})...", screen_w, screen_h);
        fb_w = screen_w;
        fb_h = screen_h;
        match create_drm_dmabuf_fallback(drm_raw_fd, fb_w, fb_h) {
            Ok((fd, _handle, pitch_val, size, ptr)) => {
                dmabuf_fd = fd;
                pitch = pitch_val;
                buf_size = size;
                buf_map = ptr;
                pixel_format = DRM_FORMAT_XRGB8888;
                unsafe {
                    let slice = std::slice::from_raw_parts_mut(buf_map as *mut u32, buf_size / 4);
                    draw_rectangles_argb(slice, fb_w, fb_h);
                }
                println!("[DMA-BUF SUCCESS] Created native DMA-BUF fd = {} ({}x{}) via PRIME export", dmabuf_fd, fb_w, fb_h);
            }
            Err(e) => {
                return Err(format!("Failed to allocate DMA-BUF: {}", e).into());
            }
        }
    }

    // -------------------------------------------------------------
    // Step 3: Import DMA-BUF fd into DRM Framebuffer
    // -------------------------------------------------------------
    println!("\n[STEP 3] Importing DMA-BUF fd ({}) into DRM Framebuffer...", dmabuf_fd);
    let mut gem_handle: u32 = 0;
    let mut fb_id: u32 = 0;

    unsafe {
        if drmPrimeFDToHandle(drm_raw_fd, dmabuf_fd as u32, &mut gem_handle) < 0 {
            return Err("drmPrimeFDToHandle failed".into());
        }
        println!("[DRM SUCCESS] Converted DMA-BUF fd ({}) -> GEM Handle ({})", dmabuf_fd, gem_handle);

        let handles: [u32; 4] = [
            gem_handle,
            if pixel_format == DRM_FORMAT_NV12 { gem_handle } else { 0 },
            0, 0,
        ];
        let pitches: [u32; 4] = [
            pitch,
            if pixel_format == DRM_FORMAT_NV12 { fb_w } else { 0 },
            0, 0,
        ];
        let offsets: [u32; 4] = [
            0,
            if pixel_format == DRM_FORMAT_NV12 { fb_w * fb_h } else { 0 },
            0, 0,
        ];

        if drmModeAddFB2(drm_raw_fd, fb_w, fb_h, pixel_format, handles.as_ptr(), pitches.as_ptr(), offsets.as_ptr(), &mut fb_id, 0) < 0 {
            return Err("drmModeAddFB2 failed".into());
        }
        println!("[DRM SUCCESS] Created DRM Framebuffer ID = {} ({}x{})", fb_id, fb_w, fb_h);
    }

    // -------------------------------------------------------------
    // Step 4: Display Framebuffer directly via DRM CRTC & Modeset
    // -------------------------------------------------------------
    println!("\n[STEP 4] Executing DRM KMS Modeset & Display on CRTC {:?}...", crtc_handle);

    let fb_handle = framebuffer::Handle::from(NonZeroU32::new(fb_id).ok_or("Invalid FB ID")?);

    card.set_crtc(crtc_handle, Some(fb_handle), (0, 0), &[conn_handle], Some(mode))?;

    println!("\n=====================================================");
    println!(" [SUCCESS] DRM KMS Display Commit Successful!");
    println!(" Screen Resolution: {}x{} @ {}Hz", screen_w, screen_h, mode.vrefresh());
    println!(" Frame Buffer Size: {}x{}", fb_w, fb_h);
    println!("=====================================================");

    println!("\nDisplaying rectangle on HDMI screen for 10 seconds...");
    thread::sleep(Duration::from_secs(10));

    unsafe {
        if fb_id != 0 { drmModeRmFB(drm_raw_fd, fb_id); }
        if !buf_map.is_null() && buf_size > 0 {
            libc::munmap(buf_map, buf_size);
        }
        if dmabuf_fd >= 0 { libc::close(dmabuf_fd); }
    }

    println!("Done.");
    Ok(())
}
