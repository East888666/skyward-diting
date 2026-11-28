/* Only explicitly preloaded into wayne's installed qseecomd. */
#define _GNU_SOURCE
#include "rpmb-compat.h"
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <stdarg.h>
#include <unistd.h>
#include <stdio.h>
#include <stdatomic.h>
static atomic_uint logged;
static int is_modern_rpmb(int fd) {
    struct stat given,expected;
    return !fstat(fd,&given) && S_ISCHR(given.st_mode) &&
        !stat("/dev/mmcblk0rpmb",&expected) && S_ISCHR(expected.st_mode) &&
        given.st_rdev==expected.st_rdev;
}
int ioctl(int fd,int request,...) {
    va_list ap; va_start(ap,request); void *arg=va_arg(ap,void *); va_end(ap);
    int prior=errno;
    int ret=(int)syscall(__NR_ioctl,fd,(unsigned int)request,arg);
    int saved=errno;
    if ((unsigned int)request!=WAYNE_MMC_IOC_RPMB || ret>=0 ||
        (saved!=EINVAL && saved!=ENOTTY) || !arg || !is_modern_rpmb(fd)) {
        errno=ret<0?saved:prior; return ret;
    }
    struct wayne_rpmb_multi multi;
    int invalid=wayne_rpmb_prepare(arg,&multi);
    if (invalid) {errno=invalid; return -1;}
    ret=(int)syscall(__NR_ioctl,fd,MMC_IOC_MULTI_CMD,&multi);
    saved=errno;
    for (size_t i=0;i<multi.count;i++) {
        struct mmc_ioc_cmd *old=arg;
        memcpy(old[i].response,multi.commands[i].response,sizeof(old[i].response));
    }
    if (ret<0 || atomic_fetch_add(&logged,1)<8) {
        dprintf(2,"wayne-rpmb: legacy -> multi commands=%llu result=%d errno=%d\n",
                (unsigned long long)multi.count,ret,ret<0?saved:0);
    }
    errno=ret<0?saved:prior;
    return ret;
}