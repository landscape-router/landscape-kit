/* An inert process behind the product's rendered Landscape service script.
 * Its only assertion is OpenRC ordering, never bpffs readiness. */
#include <stdio.h>
#include <signal.h>
#include <unistd.h>

int main(void) {
    if (access("/run/openrc/started/lkit.service", F_OK) != 0) {
        return 42;
    }
    FILE *marker = fopen("/run/landscape-order-ok", "w");
    if (!marker) {
        return 43;
    }
    fputs("lkit service start completed before Landscape\n", marker);
    fclose(marker);
    for (;;) {
        pause();
    }
}
