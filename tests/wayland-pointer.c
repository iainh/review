// Native pointer input for the disposable 1280×900 Sway test session.
// Build with the generated wlr-virtual-pointer client protocol.
#define _DEFAULT_SOURCE
#include <linux/input-event-codes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include "virtual-pointer.h"

static struct zwlr_virtual_pointer_manager_v1 *manager;

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    (void)version;
    if (!strcmp(interface, "zwlr_virtual_pointer_manager_v1")) {
        manager = wl_registry_bind(registry, name,
            &zwlr_virtual_pointer_manager_v1_interface, 1);
    }
}

static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static uint32_t now(void) {
    struct timespec time;
    clock_gettime(CLOCK_MONOTONIC, &time);
    return (uint32_t)(time.tv_sec * 1000 + time.tv_nsec / 1000000);
}

static void settle(struct wl_display *display) {
    wl_display_roundtrip(display);
    usleep(200000);
    wl_display_roundtrip(display);
}

int main(int argc, char **argv) {
    if (argc < 4 || (strcmp(argv[1], "click") && strcmp(argv[1], "double") &&
        strcmp(argv[1], "triple") && strcmp(argv[1], "right") &&
        strcmp(argv[1], "drag") && strcmp(argv[1], "scroll"))) {
        fprintf(stderr, "usage: pointer click|double|triple|right x y | drag x y x2 y2 | scroll x y delta\n");
        return 1;
    }
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    const struct wl_registry_listener listener = {global, removed};
    wl_registry_add_listener(registry, &listener, NULL);
    wl_display_roundtrip(display);
    if (!manager) {
        fprintf(stderr, "Compositor has no wlr virtual pointer protocol\n");
        return 1;
    }
    struct zwlr_virtual_pointer_v1 *pointer =
        zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    settle(display);
    zwlr_virtual_pointer_v1_motion_absolute(pointer, now(), atoi(argv[2]), atoi(argv[3]), 1280, 900);
    zwlr_virtual_pointer_v1_frame(pointer);
    settle(display);
    if (!strcmp(argv[1], "scroll") && argc == 5) {
        zwlr_virtual_pointer_v1_axis_source(pointer, WL_POINTER_AXIS_SOURCE_FINGER);
        // Begin the gesture before sending movement. egui uses the Started
        // event to initialize its touchpad gesture state.
        zwlr_virtual_pointer_v1_axis(pointer, now(), WL_POINTER_AXIS_VERTICAL_SCROLL,
            wl_fixed_from_int(1));
        zwlr_virtual_pointer_v1_frame(pointer);
        settle(display);
        zwlr_virtual_pointer_v1_axis(pointer, now(), WL_POINTER_AXIS_VERTICAL_SCROLL,
            wl_fixed_from_int(atoi(argv[4])));
        zwlr_virtual_pointer_v1_frame(pointer);
        settle(display);
        zwlr_virtual_pointer_v1_axis_stop(pointer, now(), WL_POINTER_AXIS_VERTICAL_SCROLL);
        zwlr_virtual_pointer_v1_frame(pointer);
    } else if (!strcmp(argv[1], "double") || !strcmp(argv[1], "triple")) {
        int count = !strcmp(argv[1], "double") ? 2 : 3;
        for (int i = 0; i < count; i++) {
            zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
            zwlr_virtual_pointer_v1_frame(pointer);
            wl_display_roundtrip(display);
            usleep(40000);
            zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(pointer);
            wl_display_roundtrip(display);
            usleep(60000);
        }
    } else if (!strcmp(argv[1], "right")) {
        zwlr_virtual_pointer_v1_button(pointer, now(), BTN_RIGHT, WL_POINTER_BUTTON_STATE_PRESSED);
        zwlr_virtual_pointer_v1_frame(pointer);
        settle(display);
        zwlr_virtual_pointer_v1_button(pointer, now(), BTN_RIGHT, WL_POINTER_BUTTON_STATE_RELEASED);
        zwlr_virtual_pointer_v1_frame(pointer);
        // Keep the same pointer device for a menu action, avoiding artificial
        // leave/enter events between opening the popup and clicking its item.
        if (argc == 6) {
            settle(display);
            zwlr_virtual_pointer_v1_motion_absolute(pointer, now(), atoi(argv[4]), atoi(argv[5]), 1280, 900);
            zwlr_virtual_pointer_v1_frame(pointer);
            settle(display);
            zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
            zwlr_virtual_pointer_v1_frame(pointer);
            settle(display);
            zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(pointer);
        }
    } else {
        zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
        zwlr_virtual_pointer_v1_frame(pointer);
        settle(display);
        if (!strcmp(argv[1], "drag") && argc == 6) {
            zwlr_virtual_pointer_v1_motion_absolute(pointer, now(), atoi(argv[4]), atoi(argv[5]), 1280, 900);
            zwlr_virtual_pointer_v1_frame(pointer);
            settle(display);
        }
        zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED);
        zwlr_virtual_pointer_v1_frame(pointer);
    }
    settle(display);
    zwlr_virtual_pointer_v1_destroy(pointer);
    zwlr_virtual_pointer_manager_v1_destroy(manager);
    wl_registry_destroy(registry);
    wl_display_flush(display);
    wl_display_disconnect(display);
    return 0;
}
