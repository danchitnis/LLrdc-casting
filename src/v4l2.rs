/*
 * V4L2 Module: Device selection, format negotiation, MMAP buffer mapping & DMA-BUF export
 */

use std::ffi::CString;
use std::os::unix::io::RawFd;
use std::ptr;

use libc::{c_int, c_ulong, MAP_FAILED, MAP_SHARED, O_CLOEXEC, O_NONBLOCK, O_RDWR, PROT_READ, PROT_WRITE};
use crate::drm_kms::{DRM_FORMAT_NV12, DRM_FORMAT_XRGB8888};
use crate::gfx;

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

pub struct V4l2ExportedBuffer {
    pub dmabuf_fd: RawFd,
    pub fb_w: u32,
    pub fb_h: u32,
    pub pitch: u32,
    pub pixel_format: u32,
    pub buf_map: *mut std::ffi::c_void,
    pub buf_size: usize,
}

/// Allocate V4L2 buffer and export DMA-BUF file descriptor
pub fn allocate_and_export_v4l2_buffer(
    requested_dev: &str,
    screen_w: u32,
    screen_h: u32,
) -> Result<V4l2ExportedBuffer, Box<dyn std::error::Error>> {
    let c_path = CString::new(requested_dev)?;
    let v_fd = unsafe { libc::open(c_path.as_ptr(), O_RDWR | O_NONBLOCK) };

    if v_fd < 0 {
        return Err(format!("Could not open V4L2 device: {}", requested_dev).into());
    }

    unsafe {
        let mut cap: V4l2Capability = std::mem::zeroed();
        if libc::ioctl(v_fd, VIDIOC_QUERYCAP, &mut cap) < 0 {
            libc::close(v_fd);
            return Err("VIDIOC_QUERYCAP failed".into());
        }

        let driver_str = std::str::from_utf8(&cap.driver).unwrap_or("").trim_matches('\0');
        let card_str = std::str::from_utf8(&cap.card).unwrap_or("").trim_matches('\0');
        println!("[V4L2] Driver: {}, Card: {}", driver_str, card_str);

        let buf_type = if (cap.capabilities & V4L2_CAP_VIDEO_CAPTURE_MPLANE) != 0 {
            V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
        } else {
            V4L2_BUF_TYPE_VIDEO_CAPTURE
        };

        // If M2M device, configure OUTPUT queue first
        if (cap.capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE)) != 0 {
            let mut fmt_out: V4l2Format = std::mem::zeroed();
            let is_mplane = (cap.capabilities & V4L2_CAP_VIDEO_M2M_MPLANE) != 0;
            fmt_out.type_ = if is_mplane { V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE } else { V4L2_BUF_TYPE_VIDEO_OUTPUT };

            let out_fmt = if driver_str == "rkvdec" { u32::from_le_bytes(*b"S264") } else { V4L2_PIX_FMT_NV12 };

            if is_mplane {
                fmt_out.fmt.pix_mp.width = screen_w;
                fmt_out.fmt.pix_mp.height = screen_h;
                fmt_out.fmt.pix_mp.pixelformat = out_fmt;
                fmt_out.fmt.pix_mp.num_planes = 1;
            } else {
                fmt_out.fmt.pix.width = screen_w;
                fmt_out.fmt.pix.height = screen_h;
                fmt_out.fmt.pix.pixelformat = out_fmt;
            }
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

        let fb_w = if negotiated_w > 0 { negotiated_w } else { screen_w };
        let fb_h = if negotiated_h > 0 { negotiated_h } else { screen_h };

        let pixel_format = if negotiated_fmt == V4L2_PIX_FMT_BGR32 { DRM_FORMAT_XRGB8888 } else { DRM_FORMAT_NV12 };
        let pitch = if pixel_format == DRM_FORMAT_XRGB8888 { fb_w * 4 } else { fb_w };

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
                let buf_size = if buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE { planes[0].length as usize } else { buf.length as usize };

                let mapped = libc::mmap(
                    ptr::null_mut(),
                    buf_size,
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    v_fd,
                    offset as libc::off_t,
                );

                if mapped != MAP_FAILED {
                    let mut expbuf: V4l2ExportBuffer = std::mem::zeroed();
                    expbuf.type_ = buf_type;
                    expbuf.index = 0;
                    expbuf.flags = (O_CLOEXEC | O_RDWR) as u32;

                    if libc::ioctl(v_fd, VIDIOC_EXPBUF, &mut expbuf) == 0 {
                        let dmabuf_fd = expbuf.fd;
                        println!("[V4L2 SUCCESS] Exported DMA-BUF fd = {}, size = {} bytes", dmabuf_fd, buf_size);

                        if pixel_format == DRM_FORMAT_NV12 || buf_size < (fb_w * fb_h * 4) as usize {
                            let slice = std::slice::from_raw_parts_mut(mapped as *mut u8, buf_size);
                            gfx::draw_rectangles_nv12(slice, fb_w, fb_h);
                        } else {
                            let slice = std::slice::from_raw_parts_mut(mapped as *mut u32, buf_size / 4);
                            gfx::draw_rectangles_argb(slice, fb_w, fb_h);
                        }
                        println!("[V4L2] Drawn rectangle on V4L2 DMA-BUF frame memory ({}x{}).", fb_w, fb_h);

                        libc::close(v_fd);
                        return Ok(V4l2ExportedBuffer {
                            dmabuf_fd,
                            fb_w,
                            fb_h,
                            pitch,
                            pixel_format,
                            buf_map: mapped,
                            buf_size,
                        });
                    }
                    libc::munmap(mapped, buf_size);
                }
            }
        }
        libc::close(v_fd);
    }

    Err("Failed to allocate or export V4L2 buffer".into())
}
