#ifndef WAYNE_RPMB_COMPAT_H
#define WAYNE_RPMB_COMPAT_H
#include <errno.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <linux/mmc/ioctl.h>
#define WAYNE_MMC_IOC_RPMB 0xc0d8b300U
#define WAYNE_RPMB_COMMANDS 3
struct wayne_rpmb_multi {
    uint64_t count;
    struct mmc_ioc_cmd commands[WAYNE_RPMB_COMMANDS];
};
_Static_assert(sizeof(struct mmc_ioc_cmd)==72,"unexpected MMC ABI");
_Static_assert(offsetof(struct wayne_rpmb_multi,commands)==8,"unexpected multi-command ABI");
/* Preserve buffers, reliable-write bits, responses and command ordering. */
static inline int wayne_rpmb_prepare(const struct mmc_ioc_cmd *old, struct wayne_rpmb_multi *out) {
    memset(out,0,sizeof(*out));
    for (size_t i=0;i<WAYNE_RPMB_COMMANDS;i++) {
        const struct mmc_ioc_cmd *c=&old[i];
        if (!c->opcode) break;
        if (c->is_acmd || c->arg || c->blksz!=512 || !c->blocks ||
            c->blocks>MMC_IOC_MAX_BYTES/512 || !c->data_ptr ||
            (c->opcode!=18 && c->opcode!=25) ||
            (c->opcode==18 && c->write_flag) ||
            (c->opcode==25 && !(c->write_flag&1))) return EINVAL;
        out->commands[out->count++]=*c;
    }
    return out->count ? 0 : EINVAL;
}
#endif