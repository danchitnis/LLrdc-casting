/*
 * Dynamic Resolution V4L2 Decoder / M2M -> DMA-BUF fd -> DRM Atomic Commit -> HDMI Pipeline
 * Target: Radxa Rock 5C+ / Rockchip RK3588 running Armbian
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <linux/videodev2.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <drm_fourcc.h>

// Structure to hold DRM Atomic Property IDs
struct drm_properties {
    uint32_t crtc_id;
    uint32_t conn_id;
    uint32_t plane_id;

    // CRTC props
    uint32_t crtc_active;
    uint32_t crtc_mode_id;

    // Conn props
    uint32_t conn_crtc_id;

    // Plane props
    uint32_t plane_fb_id;
    uint32_t plane_crtc_id;
    uint32_t plane_src_x;
    uint32_t plane_src_y;
    uint32_t plane_src_w;
    uint32_t plane_src_h;
    uint32_t plane_crtc_x;
    uint32_t plane_crtc_y;
    uint32_t plane_crtc_w;
    uint32_t plane_crtc_h;
};

// Helper function to get property ID by name
static uint32_t get_prop_id(int drm_fd, uint32_t obj_id, uint32_t obj_type, const char *name) {
    if (obj_id == 0) return 0;
    drmModeObjectProperties *props = drmModeObjectGetProperties(drm_fd, obj_id, obj_type);
    if (!props) return 0;

    uint32_t prop_id = 0;
    for (uint32_t i = 0; i < props->count_props; i++) {
        drmModePropertyRes *prop = drmModeGetProperty(drm_fd, props->props[i]);
        if (prop) {
            if (strcmp(prop->name, name) == 0) {
                prop_id = prop->prop_id;
                drmModeFreeProperty(prop);
                break;
            }
            drmModeFreeProperty(prop);
        }
    }
    drmModeFreeObjectProperties(props);
    return prop_id;
}

// Helper function to select the preferred or highest resolution mode on a connector
static drmModeModeInfo select_best_mode(drmModeConnectorPtr c) {
    drmModeModeInfo best_mode = {0};
    if (!c || c->count_modes == 0) return best_mode;

    // 1. Look for mode explicitly marked as PREFERRED
    for (int i = 0; i < c->count_modes; i++) {
        if (c->modes[i].type & DRM_MODE_TYPE_PREFERRED) {
            printf("[DRM] Found PREFERRED mode: %dx%d @ %dHz\n",
                   c->modes[i].hdisplay, c->modes[i].vdisplay, c->modes[i].vrefresh);
            return c->modes[i];
        }
    }

    // 2. Fallback: Find mode with maximum resolution area (width * height)
    uint32_t max_area = 0;
    for (int i = 0; i < c->count_modes; i++) {
        uint32_t area = (uint32_t)c->modes[i].hdisplay * c->modes[i].vdisplay;
        if (area > max_area) {
            max_area = area;
            best_mode = c->modes[i];
        }
    }
    printf("[DRM] Found MAX RESOLUTION mode: %dx%d @ %dHz\n",
           best_mode.hdisplay, best_mode.vdisplay, best_mode.vrefresh);
    return best_mode;
}

// Function to draw rectangles on an ARGB8888 frame buffer
static void draw_rectangles_argb(uint32_t *buf, int width, int height) {
    // Fill background with dark grey (0xFF1E1E24)
    uint32_t bg_color = 0xFF1E1E24;
    for (int i = 0; i < width * height; i++) {
        buf[i] = bg_color;
    }

    // Main central rectangle coordinates
    int rect_x = width / 4;
    int rect_y = height / 4;
    int rect_w = width / 2;
    int rect_h = height / 2;
    int border_thick = width / 160;
    if (border_thick < 4) border_thick = 4;

    uint32_t border_color = 0xFF00FFCC; // Cyan border
    uint32_t fill_color   = 0xFFFF3366; // Coral pink fill

    // Inner rectangle dimensions
    int inner_x = rect_x + border_thick;
    int inner_y = rect_y + border_thick;
    int inner_w = rect_w - (2 * border_thick);
    int inner_h = rect_h - (2 * border_thick);

    for (int y = rect_y; y < rect_y + rect_h; y++) {
        for (int x = rect_x; x < rect_x + rect_w; x++) {
            if (y >= 0 && y < height && x >= 0 && x < width) {
                if (x >= inner_x && x < inner_x + inner_w &&
                    y >= inner_y && y < inner_y + inner_h) {
                    buf[y * width + x] = fill_color;
                } else {
                    buf[y * width + x] = border_color;
                }
            }
        }
    }

    // Secondary inner box
    int box2_w = rect_w / 3;
    int box2_h = rect_h / 3;
    int box2_x = rect_x + (rect_w - box2_w) / 2;
    int box2_y = rect_y + (rect_h - box2_h) / 2;
    uint32_t box2_color = 0xFFFFCC00; // Bright yellow

    for (int y = box2_y; y < box2_y + box2_h; y++) {
        for (int x = box2_x; x < box2_x + box2_w; x++) {
            if (y >= 0 && y < height && x >= 0 && x < width) {
                buf[y * width + x] = box2_color;
            }
        }
    }
}

// Function to draw rectangles on NV12 frame buffer
static void draw_rectangles_nv12(uint8_t *buf, int width, int height) {
    int y_size = width * height;
    uint8_t *y_plane = buf;
    uint8_t *uv_plane = buf + y_size;

    // Fill background with dark gray Y=40, U=128, V=128
    memset(y_plane, 40, y_size);
    memset(uv_plane, 128, y_size / 2);

    // Draw central rectangle border & fill
    int rect_x = width / 4;
    int rect_y = height / 4;
    int rect_w = width / 2;
    int rect_h = height / 2;
    int border_thick = width / 160;
    if (border_thick < 4) border_thick = 4;

    // Cyan border: Y=170, U=166, V=16; Fill green: Y=145, U=54, V=34
    for (int r = rect_y; r < rect_y + rect_h; r++) {
        for (int c = rect_x; c < rect_x + rect_w; c++) {
            int border = (r < rect_y + border_thick || r >= rect_y + rect_h - border_thick ||
                          c < rect_x + border_thick || c >= rect_x + rect_w - border_thick);
            
            // Y component
            y_plane[r * width + c] = border ? 170 : 145;

            // UV component (subsampled 2x2)
            if ((r % 2 == 0) && (c % 2 == 0)) {
                int uv_idx = (r / 2) * width + (c & ~1);
                uv_plane[uv_idx]     = border ? 166 : 54;  // U
                uv_plane[uv_idx + 1] = border ? 16  : 34;  // V
            }
        }
    }
}

// Fallback DRM dumb buffer creation if V4L2 export is unavailable
static int create_drm_dmabuf_fallback(int drm_fd, int width, int height, uint32_t *out_handle, uint32_t *out_pitch, uint32_t *out_size, int *out_dmabuf_fd, void **out_map) {
    struct drm_mode_create_dumb create_dumb = {0};
    create_dumb.width = width;
    create_dumb.height = height;
    create_dumb.bpp = 32;

    if (drmIoctl(drm_fd, DRM_IOCTL_MODE_CREATE_DUMB, &create_dumb) < 0) {
        perror("[DRM] DRM_IOCTL_MODE_CREATE_DUMB failed");
        return -1;
    }

    *out_handle = create_dumb.handle;
    *out_pitch  = create_dumb.pitch;
    *out_size   = create_dumb.size;

    // Export DRM dumb buffer handle to DMA-BUF file descriptor
    struct drm_prime_handle prime = {0};
    prime.handle = create_dumb.handle;
    prime.flags  = DRM_CLOEXEC | DRM_RDWR;

    if (drmIoctl(drm_fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &prime) < 0) {
        perror("[DRM] DRM_IOCTL_PRIME_HANDLE_TO_FD failed");
        return -1;
    }

    *out_dmabuf_fd = prime.fd;

    // Map dumb buffer for CPU access
    struct drm_mode_map_dumb map_dumb = {0};
    map_dumb.handle = create_dumb.handle;
    if (drmIoctl(drm_fd, DRM_IOCTL_MODE_MAP_DUMB, &map_dumb) < 0) {
        perror("[DRM] DRM_IOCTL_MODE_MAP_DUMB failed");
        return -1;
    }

    *out_map = mmap(NULL, create_dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, drm_fd, map_dumb.offset);
    if (*out_map == MAP_FAILED) {
        perror("[DRM] mmap dumb buffer failed");
        return -1;
    }

    return 0;
}

int main(int argc, char *argv[]) {
    setbuf(stdout, NULL);
    setbuf(stderr, NULL);

    printf("=====================================================\n");
    printf(" V4L2 Decoder -> DMA-BUF fd -> DRM Atomic Commit -> HDMI\n");
    printf(" Radxa Rock 5C+ / Rockchip RK3588 DRM Display\n");
    printf(" Dynamic Resolution Autodetection\n");
    printf("=====================================================\n\n");

    const char *v4l2_dev_name = "/dev/video0";
    if (argc > 1) v4l2_dev_name = argv[1];

    // -------------------------------------------------------------
    // Step 1: Open DRM Card Device & Autodetect HDMI Screen Resolution
    // -------------------------------------------------------------
    printf("[STEP 1] Opening DRM device and autodetecting display resolution...\n");
    int drm_fd = -1;
    char card_path[32];
    for (int i = 0; i < 4; i++) {
        snprintf(card_path, sizeof(card_path), "/dev/dri/card%d", i);
        drm_fd = open(card_path, O_RDWR | O_CLOEXEC);
        if (drm_fd >= 0) {
            drmVersionPtr version = drmGetVersion(drm_fd);
            if (version) {
                if (strcmp(version->name, "rockchip") == 0 || strcmp(version->name, "panfrost") != 0) {
                    printf("[DRM] Selected display card: %s (Driver: %s)\n", card_path, version->name);
                    drmFreeVersion(version);
                    break;
                }
                drmFreeVersion(version);
            }
            close(drm_fd);
            drm_fd = -1;
        }
    }

    if (drm_fd < 0) {
        fprintf(stderr, "[ERROR] Could not open DRM card device!\n");
        return EXIT_FAILURE;
    }

    // Enable DRM client capabilities for Atomic KMS
    if (drmSetClientCap(drm_fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1) < 0 ||
        drmSetClientCap(drm_fd, DRM_CLIENT_CAP_ATOMIC, 1) < 0) {
        fprintf(stderr, "[DRM] Failed to set DRM_CLIENT_CAP_ATOMIC\n");
    }

    drmModeResPtr res = drmModeGetResources(drm_fd);
    if (!res) {
        fprintf(stderr, "[DRM ERROR] Failed to get DRM resources\n");
        close(drm_fd);
        return EXIT_FAILURE;
    }

    uint32_t conn_id = 0;
    uint32_t crtc_id = 0;
    drmModeModeInfo mode = {0};

    // Find connected HDMI connector and query best resolution
    for (int i = 0; i < res->count_connectors; i++) {
        drmModeConnectorPtr c = drmModeGetConnector(drm_fd, res->connectors[i]);
        if (!c) continue;

        if (c->connection == DRM_MODE_CONNECTED &&
            (c->connector_type == DRM_MODE_CONNECTOR_HDMIA || c->connector_type == DRM_MODE_CONNECTOR_HDMIB)) {
            conn_id = c->connector_id;
            mode = select_best_mode(c);
            drmModeFreeConnector(c);
            break;
        }
        drmModeFreeConnector(c);
    }

    // Fallback: Check any connected connector
    if (!conn_id) {
        for (int i = 0; i < res->count_connectors; i++) {
            drmModeConnectorPtr c = drmModeGetConnector(drm_fd, res->connectors[i]);
            if (c && c->connection == DRM_MODE_CONNECTED && c->count_modes > 0) {
                conn_id = c->connector_id;
                mode = select_best_mode(c);
                drmModeFreeConnector(c);
                break;
            }
            if (c) drmModeFreeConnector(c);
        }
    }

    // Default synthetic mode if no active mode returned
    if (mode.hdisplay == 0) {
        mode.clock = 148500;
        mode.hdisplay = 1920;
        mode.hsync_start = 2008;
        mode.hsync_end = 2052;
        mode.htotal = 2200;
        mode.vdisplay = 1080;
        mode.vsync_start = 1084;
        mode.vsync_end = 1089;
        mode.vtotal = 1125;
        mode.vrefresh = 60;
        mode.flags = DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_PVSYNC;
        snprintf(mode.name, sizeof(mode.name), "1920x1080");
    }

    uint32_t screen_w = mode.hdisplay;
    uint32_t screen_h = mode.vdisplay;

    printf("[DRM AUTODETECT SUCCESS] Screen Resolution: %ux%u @ %uHz (Connector ID: %u)\n",
           screen_w, screen_h, mode.vrefresh, conn_id);

    // Pick CRTC
    if (res->count_crtcs > 0) {
        crtc_id = res->crtcs[0];
    }
    drmModeFreeResources(res);

    // Find Universal/Primary Plane
    drmModePlaneResPtr plane_res = drmModeGetPlaneResources(drm_fd);
    uint32_t plane_id = 0;
    if (plane_res) {
        for (uint32_t i = 0; i < plane_res->count_planes; i++) {
            drmModePlanePtr plane = drmModeGetPlane(drm_fd, plane_res->planes[i]);
            if (plane) {
                plane_id = plane->plane_id;
                drmModeFreePlane(plane);
                break;
            }
        }
        drmModeFreePlaneResources(plane_res);
    }

    // -------------------------------------------------------------
    // Step 2: V4L2 Buffer Allocation with Autodetected Resolution & DMA-BUF Export
    // -------------------------------------------------------------
    printf("\n[STEP 2] Opening V4L2 device and setting target %ux%u resolution...\n", screen_w, screen_h);
    
    char best_v4l2_dev[32];
    snprintf(best_v4l2_dev, sizeof(best_v4l2_dev), "%s", v4l2_dev_name);

    int v4l2_fd = -1;
    int v4l2_success = 0;
    int dmabuf_fd = -1;
    void *buf_map = NULL;
    size_t buf_size = 0;
    uint32_t fb_w = screen_w;
    uint32_t fb_h = screen_h;
    uint32_t pixel_format = DRM_FORMAT_NV12;
    uint32_t pitch = fb_w;

    // Probe V4L2 video nodes to find the node accepting target screen resolution
    if (argc <= 1) {
        uint32_t best_w = 0;
        for (int dev_idx = 0; dev_idx < 10; dev_idx++) {
            char dev_path[32];
            snprintf(dev_path, sizeof(dev_path), "/dev/video%d", dev_idx);
            int probe_fd = open(dev_path, O_RDWR | O_NONBLOCK, 0);
            if (probe_fd < 0) continue;

            struct v4l2_capability cap;
            if (ioctl(probe_fd, VIDIOC_QUERYCAP, &cap) == 0) {
                if (cap.capabilities & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE | V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE)) {
                    uint32_t formats[] = { V4L2_PIX_FMT_NV12, V4L2_PIX_FMT_BGR32 };
                    for (size_t f = 0; f < sizeof(formats)/sizeof(formats[0]); f++) {
                        // For M2M devices, configure OUTPUT queue first
                        if (cap.capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE)) {
                            struct v4l2_format fmt_out = {0};
                            fmt_out.type = (cap.capabilities & V4L2_CAP_VIDEO_M2M_MPLANE) ?
                                           V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE : V4L2_BUF_TYPE_VIDEO_OUTPUT;
                            if (fmt_out.type == V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) {
                                fmt_out.fmt.pix_mp.width = screen_w;
                                fmt_out.fmt.pix_mp.height = screen_h;
                                fmt_out.fmt.pix_mp.pixelformat = formats[f];
                                fmt_out.fmt.pix_mp.num_planes = 1;
                            } else {
                                fmt_out.fmt.pix.width = screen_w;
                                fmt_out.fmt.pix.height = screen_h;
                                fmt_out.fmt.pix.pixelformat = formats[f];
                            }
                            ioctl(probe_fd, VIDIOC_S_FMT, &fmt_out);
                        }

                        uint32_t buf_type = (cap.capabilities & V4L2_CAP_VIDEO_CAPTURE_MPLANE) ?
                                             V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE : V4L2_BUF_TYPE_VIDEO_CAPTURE;
                        struct v4l2_format fmt = {0};
                        fmt.type = buf_type;
                        if (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
                            fmt.fmt.pix_mp.width = screen_w;
                            fmt.fmt.pix_mp.height = screen_h;
                            fmt.fmt.pix_mp.pixelformat = formats[f];
                            fmt.fmt.pix_mp.num_planes = 1;
                        } else {
                            fmt.fmt.pix.width = screen_w;
                            fmt.fmt.pix.height = screen_h;
                            fmt.fmt.pix.pixelformat = formats[f];
                        }
                        if (ioctl(probe_fd, VIDIOC_S_FMT, &fmt) == 0 || ioctl(probe_fd, VIDIOC_TRY_FMT, &fmt) == 0) {
                            uint32_t try_w = (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) ? fmt.fmt.pix_mp.width : fmt.fmt.pix.width;
                            if (try_w > best_w) {
                                best_w = try_w;
                                snprintf(best_v4l2_dev, sizeof(best_v4l2_dev), "%s", dev_path);
                                if (try_w >= screen_w) {
                                    break;
                                }
                            }
                        }
                    }
                    if (best_w >= screen_w) {
                        close(probe_fd);
                        break;
                    }
                }
            }
            close(probe_fd);
        }
    }

    printf("[V4L2] Selected V4L2 device node: %s\n", best_v4l2_dev);
    v4l2_fd = open(best_v4l2_dev, O_RDWR | O_NONBLOCK, 0);

    if (v4l2_fd >= 0) {
        struct v4l2_capability cap;
        if (ioctl(v4l2_fd, VIDIOC_QUERYCAP, &cap) == 0) {
            printf("[V4L2] Driver: %s, Card: %s\n", cap.driver, cap.card);

            uint32_t buf_type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if (cap.capabilities & V4L2_CAP_VIDEO_CAPTURE_MPLANE) {
                buf_type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
            }

            // If M2M device, configure OUTPUT queue first
            if (cap.capabilities & (V4L2_CAP_VIDEO_M2M | V4L2_CAP_VIDEO_M2M_MPLANE)) {
                struct v4l2_format fmt_out = {0};
                fmt_out.type = (cap.capabilities & V4L2_CAP_VIDEO_M2M_MPLANE) ?
                               V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE : V4L2_BUF_TYPE_VIDEO_OUTPUT;
                fmt_out.fmt.pix.width       = screen_w;
                fmt_out.fmt.pix.height      = screen_h;
                fmt_out.fmt.pix.pixelformat = V4L2_PIX_FMT_NV12;
                fmt_out.fmt.pix.field       = V4L2_FIELD_ANY;
                ioctl(v4l2_fd, VIDIOC_S_FMT, &fmt_out);
            }

            // Configure CAPTURE queue format matching autodetected resolution
            struct v4l2_format fmt = {0};
            fmt.type = buf_type;

            if (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
                fmt.fmt.pix_mp.width        = screen_w;
                fmt.fmt.pix_mp.height       = screen_h;
                fmt.fmt.pix_mp.pixelformat  = V4L2_PIX_FMT_NV12;
                fmt.fmt.pix_mp.field        = V4L2_FIELD_ANY;
                fmt.fmt.pix_mp.num_planes   = 1;
            } else {
                fmt.fmt.pix.width           = screen_w;
                fmt.fmt.pix.height          = screen_h;
                fmt.fmt.pix.pixelformat     = V4L2_PIX_FMT_NV12;
                fmt.fmt.pix.field           = V4L2_FIELD_ANY;
            }

            if (ioctl(v4l2_fd, VIDIOC_S_FMT, &fmt) < 0) {
                if (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
                    fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_BGR32;
                } else {
                    fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_BGR32;
                }
                ioctl(v4l2_fd, VIDIOC_S_FMT, &fmt);
            }

            // Read negotiated dimensions
            uint32_t negotiated_w = (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) ? fmt.fmt.pix_mp.width : fmt.fmt.pix.width;
            uint32_t negotiated_h = (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) ? fmt.fmt.pix_mp.height : fmt.fmt.pix.height;
            uint32_t negotiated_fmt = (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) ? fmt.fmt.pix_mp.pixelformat : fmt.fmt.pix.pixelformat;

            if (negotiated_w > 0 && negotiated_h > 0) {
                fb_w = negotiated_w;
                fb_h = negotiated_h;
            }

            if (negotiated_fmt == V4L2_PIX_FMT_BGR32) {
                pixel_format = DRM_FORMAT_XRGB8888;
                pitch = fb_w * 4;
            } else {
                pixel_format = DRM_FORMAT_NV12;
                pitch = fb_w;
            }
            printf("[V4L2] Negotiated format: %s (%ux%u), pitch: %u\n",
                   pixel_format == DRM_FORMAT_NV12 ? "NV12" : "XRGB8888", fb_w, fb_h, pitch);

            // Request MMAP buffers
            struct v4l2_requestbuffers reqbuf = {0};
            reqbuf.count  = 1;
            reqbuf.type   = buf_type;
            reqbuf.memory = V4L2_MEMORY_MMAP;

            if (ioctl(v4l2_fd, VIDIOC_REQBUFS, &reqbuf) == 0 && reqbuf.count > 0) {
                struct v4l2_buffer buf = {0};
                struct v4l2_plane planes[1] = {{0}};
                buf.type   = buf_type;
                buf.memory = V4L2_MEMORY_MMAP;
                buf.index  = 0;

                if (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
                    buf.m.planes = planes;
                    buf.length = 1;
                }

                if (ioctl(v4l2_fd, VIDIOC_QUERYBUF, &buf) == 0) {
                    size_t offset = (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) ? planes[0].m.mem_offset : buf.m.offset;
                    buf_size = (buf_type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) ? planes[0].length : buf.length;

                    buf_map = mmap(NULL, buf_size, PROT_READ | PROT_WRITE, MAP_SHARED, v4l2_fd, offset);

                    if (buf_map != MAP_FAILED) {
                        // Export V4L2 buffer to DMA-BUF fd
                        struct v4l2_exportbuffer expbuf = {0};
                        expbuf.type  = buf_type;
                        expbuf.index = 0;
                        expbuf.flags = O_CLOEXEC | O_RDWR;

                        if (ioctl(v4l2_fd, VIDIOC_EXPBUF, &expbuf) == 0) {
                            dmabuf_fd = expbuf.fd;
                            v4l2_success = 1;
                            printf("[V4L2 SUCCESS] Exported DMA-BUF fd = %d, size = %zu bytes\n", dmabuf_fd, buf_size);

                            // Draw rectangle dynamically on V4L2 mapped buffer
                            if (pixel_format == DRM_FORMAT_NV12 || buf_size < (size_t)(fb_w * fb_h * 4)) {
                                draw_rectangles_nv12((uint8_t *)buf_map, fb_w, fb_h);
                            } else {
                                draw_rectangles_argb((uint32_t *)buf_map, fb_w, fb_h);
                            }
                            printf("[V4L2] Drawn rectangle on V4L2 DMA-BUF frame memory (%ux%u).\n", fb_w, fb_h);
                        } else {
                            perror("[V4L2] VIDIOC_EXPBUF failed");
                        }
                    }
                }
            }
        }
    }

    // Fallback if V4L2 export failed or if V4L2 clamped resolution below full screen resolution
    uint32_t dumb_handle = 0;
    if (!v4l2_success || dmabuf_fd < 0 || fb_w < screen_w) {
        if (v4l2_success && fb_w < screen_w) {
            printf("[INFO] V4L2 hardware node clamped buffer to %ux%u; allocating native 1:1 screen resolution (%ux%u) DMA-BUF...\n",
                   fb_w, fb_h, screen_w, screen_h);
            if (buf_map && buf_size) munmap(buf_map, buf_size);
            if (dmabuf_fd >= 0) close(dmabuf_fd);
            v4l2_success = 0;
            dmabuf_fd = -1;
        }

        printf("[INFO] Allocating native DRM PRIME DMA-BUF buffer (%ux%u)...\n", screen_w, screen_h);
        uint32_t dumb_size = 0;
        fb_w = screen_w;
        fb_h = screen_h;
        if (create_drm_dmabuf_fallback(drm_fd, fb_w, fb_h, &dumb_handle, &pitch, &dumb_size, &dmabuf_fd, &buf_map) < 0) {
            fprintf(stderr, "[ERROR] Failed to allocate DMA-BUF!\n");
            close(drm_fd);
            return EXIT_FAILURE;
        }
        buf_size = dumb_size;
        pixel_format = DRM_FORMAT_XRGB8888;
        draw_rectangles_argb((uint32_t *)buf_map, fb_w, fb_h);
        printf("[DMA-BUF SUCCESS] Created native DMA-BUF fd = %d (%ux%u) via PRIME export\n", dmabuf_fd, fb_w, fb_h);
    }

    // -------------------------------------------------------------
    // Step 3: Import DMA-BUF fd into DRM Framebuffer
    // -------------------------------------------------------------
    printf("\n[STEP 3] Importing DMA-BUF fd (%d) into DRM Framebuffer...\n", dmabuf_fd);
    uint32_t gem_handle = 0;
    if (drmPrimeFDToHandle(drm_fd, dmabuf_fd, &gem_handle) < 0) {
        perror("[DRM] drmPrimeFDToHandle failed");
        if (v4l2_fd >= 0) close(v4l2_fd);
        close(drm_fd);
        return EXIT_FAILURE;
    }
    printf("[DRM SUCCESS] Converted DMA-BUF fd (%d) -> GEM Handle (%u)\n", dmabuf_fd, gem_handle);

    uint32_t fb_id = 0;
    uint32_t handles[4] = { gem_handle, 0, 0, 0 };
    uint32_t pitches[4] = { pitch, 0, 0, 0 };
    uint32_t offsets[4] = { 0, 0, 0, 0 };

    if (pixel_format == DRM_FORMAT_NV12) {
        handles[1] = gem_handle;
        pitches[1] = fb_w;
        offsets[1] = fb_w * fb_h;
    }

    int ret = drmModeAddFB2(drm_fd, fb_w, fb_h, pixel_format, handles, pitches, offsets, &fb_id, 0);
    if (ret < 0) {
        perror("[DRM] drmModeAddFB2 failed");
        if (v4l2_fd >= 0) close(v4l2_fd);
        close(drm_fd);
        return EXIT_FAILURE;
    }
    printf("[DRM SUCCESS] Created DRM Framebuffer ID = %u (%ux%u)\n", fb_id, fb_w, fb_h);

    // -------------------------------------------------------------
    // Step 4: DRM Atomic Commit to HDMI Display
    // -------------------------------------------------------------
    printf("\n[STEP 4] Executing DRM Atomic Commit on Connector %u (CRTC %u, Plane %u)...\n",
           conn_id, crtc_id, plane_id);

    // Create property blob for display mode
    uint32_t mode_blob_id = 0;
    if (drmModeCreatePropertyBlob(drm_fd, &mode, sizeof(mode), &mode_blob_id) < 0) {
        perror("[DRM] Failed to create mode property blob");
    }

    // Retrieve Property IDs
    struct drm_properties props = {0};
    props.crtc_id      = crtc_id;
    props.conn_id      = conn_id;
    props.plane_id     = plane_id;

    props.crtc_active  = get_prop_id(drm_fd, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE");
    props.crtc_mode_id = get_prop_id(drm_fd, crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID");
    props.conn_crtc_id = get_prop_id(drm_fd, conn_id, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID");

    props.plane_fb_id  = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID");
    props.plane_crtc_id= get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_ID");
    props.plane_src_x  = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_X");
    props.plane_src_y  = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_Y");
    props.plane_src_w  = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_W");
    props.plane_src_h  = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_H");
    props.plane_crtc_x = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_X");
    props.plane_crtc_y = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_Y");
    props.plane_crtc_w = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_W");
    props.plane_crtc_h = get_prop_id(drm_fd, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_H");

    // Build Atomic Request
    drmModeAtomicReq *req = drmModeAtomicAlloc();
    if (!req) {
        fprintf(stderr, "[DRM ERROR] Failed to allocate DRM atomic request\n");
        return EXIT_FAILURE;
    }

    if (props.conn_crtc_id) drmModeAtomicAddProperty(req, conn_id, props.conn_crtc_id, crtc_id);
    if (props.crtc_active)  drmModeAtomicAddProperty(req, crtc_id, props.crtc_active, 1);
    if (props.crtc_mode_id && mode_blob_id) drmModeAtomicAddProperty(req, crtc_id, props.crtc_mode_id, mode_blob_id);

    if (props.plane_fb_id)   drmModeAtomicAddProperty(req, plane_id, props.plane_fb_id, fb_id);
    if (props.plane_crtc_id) drmModeAtomicAddProperty(req, plane_id, props.plane_crtc_id, crtc_id);
    if (props.plane_crtc_x)  drmModeAtomicAddProperty(req, plane_id, props.plane_crtc_x, 0);
    if (props.plane_crtc_y)  drmModeAtomicAddProperty(req, plane_id, props.plane_crtc_y, 0);
    if (props.plane_crtc_w)  drmModeAtomicAddProperty(req, plane_id, props.plane_crtc_w, screen_w);
    if (props.plane_crtc_h)  drmModeAtomicAddProperty(req, plane_id, props.plane_crtc_h, screen_h);

    if (props.plane_src_x)   drmModeAtomicAddProperty(req, plane_id, props.plane_src_x, 0 << 16);
    if (props.plane_src_y)   drmModeAtomicAddProperty(req, plane_id, props.plane_src_y, 0 << 16);
    if (props.plane_src_w)   drmModeAtomicAddProperty(req, plane_id, props.plane_src_w, fb_w << 16);
    if (props.plane_src_h)   drmModeAtomicAddProperty(req, plane_id, props.plane_src_h, fb_h << 16);

    uint32_t flags = DRM_MODE_ATOMIC_ALLOW_MODESET;
    ret = drmModeAtomicCommit(drm_fd, req, flags, NULL);

    if (ret < 0) {
        perror("[DRM ERROR] drmModeAtomicCommit failed");
        ret = drmModeSetCrtc(drm_fd, crtc_id, fb_id, 0, 0, &conn_id, 1, &mode);
        if (ret == 0) {
            printf("[DRM SUCCESS] Rendered frame via drmModeSetCrtc fallback!\n");
        } else {
            perror("[DRM ERROR] drmModeSetCrtc fallback failed");
        }
    } else {
        printf("\n=====================================================\n");
        printf(" [SUCCESS] DRM Atomic Commit Successful!\n");
        printf(" Screen Resolution: %ux%u @ %uHz\n", screen_w, screen_h, mode.vrefresh);
        printf(" Frame Buffer Size: %ux%u\n", fb_w, fb_h);
        printf("=====================================================\n");
    }

    printf("\nDisplaying rectangle on HDMI screen for 10 seconds...\n");
    sleep(10);

    // Cleanup
    if (req) drmModeAtomicFree(req);
    if (mode_blob_id) drmModeDestroyPropertyBlob(drm_fd, mode_blob_id);
    if (fb_id) drmModeRmFB(drm_fd, fb_id);
    if (buf_map && buf_size) munmap(buf_map, buf_size);
    if (dmabuf_fd >= 0) close(dmabuf_fd);
    if (v4l2_fd >= 0) close(v4l2_fd);
    if (drm_fd >= 0) close(drm_fd);

    printf("Done.\n");
    return EXIT_SUCCESS;
}
