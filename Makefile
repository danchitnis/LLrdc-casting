CC = gcc
CFLAGS = -Wall -Wextra -O2 $(shell pkg-config --cflags libdrm libv4l2)
LIBS = $(shell pkg-config --libs libdrm libv4l2)

TARGET = v4l2_dmabuf_drm
SRCS = src/v4l2_dmabuf_drm.c

all: $(TARGET)

$(TARGET): $(SRCS)
	$(CC) $(CFLAGS) -o $@ $(SRCS) $(LIBS)

clean:
	rm -f $(TARGET)

.PHONY: all clean
