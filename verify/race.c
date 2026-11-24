#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#include <limits.h>
#include <unistd.h>
/* Stands in for another program: writes "theirs" to `path` just before the real
   call, once, if a file called .race is in the same folder. */
static void race(const char *path) {
    char marker[PATH_MAX];
    size_t n = strlen(path);
    if (n < 3 || strcmp(path + n - 3, ".md") || n + 8 > sizeof marker) return;
    strcpy(marker, path);
    char *slash = strrchr(marker, '/');
    if (!slash) return;
    strcpy(slash + 1, ".race");
    if (access(marker, F_OK)) return;
    int (*real_unlink)(const char *) = dlsym(RTLD_NEXT, "unlink");
    real_unlink(marker);
    FILE *f = fopen(path, "w");
    if (f) { fputs("theirs", f); fclose(f); }
}
int rename(const char *from, const char *to) {
    static int (*real)(const char *, const char *);
    if (!real) real = dlsym(RTLD_NEXT, "rename");
    race(to);
    return real(from, to);
}
int unlink(const char *path) {
    static int (*real)(const char *);
    if (!real) real = dlsym(RTLD_NEXT, "unlink");
    race(path);
    return real(path);
}
