#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/sysinfo.h>
#include <unistd.h>

#define CONFIG "25-swap.conf"
#define LABEL "losos-swap"
#define GRAIN UINT64_C(4096)
#ifndef SYS_BLOCK
#define SYS_BLOCK "/sys/class/block"
#endif

/* This runs as root in the initrd, as systemd-repart's ExecStartPre= with
 * /run/repart.d (disk.nix), where nothing else writes: no ownership checks,
 * no symlink-safe walk and no retrying a staging name are needed. */

/* Whether any partition is already named losos-swap. The kernel reads the
 * GPT names when it scans a disk and lists them in sysfs as PARTNAME=, so
 * this needs no udev and cannot race the by-partlabel links. */
static int swap_partition_exists(void)
{
    static const char wanted[] = "\nPARTNAME=" LABEL "\n";
    DIR *block = opendir(SYS_BLOCK);
    if (!block)
        return 0;
    int found = 0;
    for (struct dirent *entry; !found && (entry = readdir(block));) {
        char path[512], text[4096 + 2] = "\n";
        if (entry->d_name[0] == '.')
            continue;
        snprintf(path, sizeof(path), "%s/%s/uevent", SYS_BLOCK, entry->d_name);
        int fd = open(path, O_RDONLY | O_CLOEXEC);
        if (fd < 0)
            continue;
        ssize_t got = read(fd, text + 1, sizeof(text) - 2);
        close(fd);
        if (got > 0) {
            text[got + 1] = '\0';
            found = strstr(text, wanted) != NULL;
        }
    }
    closedir(block);
    return found;
}

int main(int argc, char **argv)
{
    struct sysinfo info;
    if (argc != 2 || !argv[1][0]) {
        fprintf(stderr, "usage: losos-swap OUTPUT-DIRECTORY\n");
        return EXIT_FAILURE;
    }
    if (sysinfo(&info) < 0) {
        perror("losos-swap: sysinfo");
        return EXIT_FAILURE;
    }
    /* totalram is usable RAM, not the momentary free RAM, rounded up to a
     * whole page. */
    uint64_t bytes = (uint64_t)info.totalram * info.mem_unit;
    bytes = (bytes + GRAIN - 1) / GRAIN * GRAIN;

    if (mkdir(argv[1], 0755) < 0 && errno != EEXIST) {
        perror("losos-swap: output directory");
        return EXIT_FAILURE;
    }
    char path[4096], staging[4096];
    snprintf(path, sizeof(path), "%s/" CONFIG, argv[1]);
    /* The hidden name does not end in .conf, so repart never reads it. */
    snprintf(staging, sizeof(staging), "%s/." CONFIG ".tmp", argv[1]);
    /* A config already in /run wins over the generated default: leave any
     * administrator override in place instead of rewriting it on each boot. */
    if (access(path, F_OK) == 0)
        return EXIT_SUCCESS;

    FILE *out = fopen(staging, "w");
    if (!out) {
        perror("losos-swap: write " CONFIG);
        return EXIT_FAILURE;
    }
    /* The first boot makes swap exactly the size of RAM. Once it exists, RAM
     * is only a cap: repart never shrinks a partition, so its minimum is the
     * larger of SizeMinBytes= and its current size, and totalram moves by
     * about a megabyte from one boot to the next. A minimum of it made swap
     * need to grow on any later boot that saw more RAM than the first, or
     * after a RAM upgrade, with root right behind it and no free space, so
     * repart failed and the boot was never counted good. */
    fputs("[Partition]\nType=swap\nLabel=" LABEL "\nFormat=swap\n", out);
    if (!swap_partition_exists())
        fprintf(out, "SizeMinBytes=%" PRIu64 "\n", bytes);
    fprintf(out, "SizeMaxBytes=%" PRIu64 "\n", bytes);
    /* Written in full and synced before the rename publishes it, so repart
     * never reads half a file. */
    if (fflush(out) != 0 || fsync(fileno(out)) < 0 || fclose(out) != 0 ||
        rename(staging, path) < 0) {
        perror("losos-swap: write " CONFIG);
        unlink(staging);
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
