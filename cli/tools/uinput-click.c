#include <linux/uinput.h>
#include <linux/input.h>
#include <sys/ioctl.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

static int pos(int *x, int *y) {
    FILE *f = popen("hyprctl cursorpos 2>/dev/null", "r");
    if (!f) return -1;
    int n = fscanf(f, "%d, %d", x, y);
    pclose(f);
    return n == 2 ? 0 : -1;
}
static void emit(int fd, int type, int code, int val) {
    struct input_event ev; memset(&ev, 0, sizeof ev);
    ev.type = type; ev.code = code; ev.value = val;
    write(fd, &ev, sizeof ev);
}
int main(int argc, char **argv) {
    int tx = argc > 1 ? atoi(argv[1]) : 0;
    int ty = argc > 2 ? atoi(argv[2]) : 0;
    int click = argc > 3 ? atoi(argv[3]) : 1;
    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) { fprintf(stderr, "open uinput: %s\n", strerror(errno)); return 2; }
    ioctl(fd, UI_SET_EVBIT, EV_KEY); ioctl(fd, UI_SET_KEYBIT, BTN_LEFT);
    ioctl(fd, UI_SET_EVBIT, EV_REL); ioctl(fd, UI_SET_RELBIT, REL_X); ioctl(fd, UI_SET_RELBIT, REL_Y);
    struct uinput_setup us; memset(&us, 0, sizeof us);
    us.id.bustype = BUS_USB; us.id.vendor = 0x1209; us.id.product = 0x0001; us.id.version = 1;
    snprintf(us.name, UINPUT_MAX_NAME_SIZE, "penelope-uinput");
    ioctl(fd, UI_DEV_SETUP, &us); ioctl(fd, UI_DEV_CREATE);
    usleep(200000);
    for (int i = 0; i < 6; i++) {
        int cx, cy;
        if (pos(&cx, &cy) != 0) break;
        int dx = tx - cx, dy = ty - cy;
        if (abs(dx) <= 2 && abs(dy) <= 2) break;
        emit(fd, EV_REL, REL_X, dx); emit(fd, EV_REL, REL_Y, dy); emit(fd, EV_SYN, SYN_REPORT, 0);
        usleep(90000);
    }
    if (click) {
        emit(fd, EV_KEY, BTN_LEFT, 1); emit(fd, EV_SYN, SYN_REPORT, 0); usleep(40000);
        emit(fd, EV_KEY, BTN_LEFT, 0); emit(fd, EV_SYN, SYN_REPORT, 0); usleep(40000);
    }
    ioctl(fd, UI_DEV_DESTROY); close(fd);
    return 0;
}
