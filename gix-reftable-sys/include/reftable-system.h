#ifndef GIX_REFTABLE_SYSTEM_H
#define GIX_REFTABLE_SYSTEM_H

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <zlib.h>

#ifdef _WIN32
#include <direct.h>
#include <io.h>
#include <windows.h>

#ifdef _MSC_VER
#ifndef _SSIZE_T_DEFINED
typedef intptr_t ssize_t;
#define _SSIZE_T_DEFINED
#endif
#endif

struct gix_reftable_dir;
typedef struct gix_reftable_dir DIR;
struct dirent {
	char d_name[4 * MAX_PATH];
};

int gix_reftable_open(const char *path, int flags);
int gix_reftable_open_with_mode(const char *path, int flags, int mode);
int gix_reftable_stat(const char *path, struct _stat64 *out);
int gix_reftable_unlink(const char *path);
int gix_reftable_rename(const char *from, const char *to);
int gix_reftable_chmod(const char *path, int mode);
DIR *gix_reftable_opendir(const char *path);
struct dirent *gix_reftable_readdir(DIR *dir);
int gix_reftable_closedir(DIR *dir);

#define open gix_reftable_open
#define close _close
#define read _read
#define write _write
#define lseek _lseeki64
#define fstat _fstat64
#define stat _stat64
#define _stat64(path, out) gix_reftable_stat(path, out)
#define unlink gix_reftable_unlink
#define rename gix_reftable_rename
#define chmod gix_reftable_chmod
#define opendir gix_reftable_opendir
#define readdir gix_reftable_readdir
#define closedir gix_reftable_closedir

int gix_reftable_sleep_ms(int timeout_ms);
#define poll(fds, count, timeout_ms) gix_reftable_sleep_ms(timeout_ms)
#else
#include <dirent.h>
#include <poll.h>
#include <sys/mman.h>
#include <unistd.h>
#endif

int reftable_fsync(int fd);
#define fsync(fd) reftable_fsync(fd)

#endif
