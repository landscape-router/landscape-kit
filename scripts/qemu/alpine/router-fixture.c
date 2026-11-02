/* An inert process behind the product's rendered Landscape service script.
 * Asserts daemon and Cgroup ordering, never bpffs readiness. */
#include <stdio.h>
#include <signal.h>
#include <string.h>
#include <unistd.h>

int main(void) {
    if (access("/run/openrc/started/lkit.service", F_OK) != 0) {
        return 42;
    }
    if (access("/run/openrc/started/cgroups", F_OK) != 0) {
        return 44;
    }
    FILE *controllers = fopen("/sys/fs/cgroup/cgroup.controllers", "r");
    if (!controllers) {
        return 45;
    }
    char controller[64];
    int cpu_ready = 0;
    while (fscanf(controllers, "%63s", controller) == 1) {
        if (strcmp(controller, "cpu") == 0) {
            cpu_ready = 1;
        }
    }
    fclose(controllers);
    if (!cpu_ready) {
        return 46;
    }
    FILE *marker = fopen("/run/landscape-order-ok", "w");
    if (!marker) {
        return 43;
    }
    fputs("lkit and cgroups service start completed before Landscape\n", marker);
    fclose(marker);
    for (;;) {
        pause();
    }
}
