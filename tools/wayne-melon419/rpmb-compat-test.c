#include "rpmb-compat.h"
#include <assert.h>
#include <stdio.h>
int main(void) {
    struct mmc_ioc_cmd old[3]={0};
    struct wayne_rpmb_multi out;
    assert(wayne_rpmb_prepare(old,&out)==EINVAL);
    old[0]=(struct mmc_ioc_cmd){.write_flag=1,.opcode=25,.blksz=512,.blocks=1,.data_ptr=0x1000};
    old[1]=(struct mmc_ioc_cmd){.opcode=18,.blksz=512,.blocks=4,.data_ptr=0x2000};
    assert(!wayne_rpmb_prepare(old,&out) && out.count==2);
    assert(!memcmp(out.commands,old,2*sizeof(old[0])));
    old[0].write_flag=0x80000001;
    old[2]=old[1];
    old[1]=old[0]; old[1].data_ptr=0x3000;
    assert(!wayne_rpmb_prepare(old,&out) && out.count==3);
    assert(!memcmp(out.commands,old,sizeof(old)));
    old[2].opcode=6; assert(wayne_rpmb_prepare(old,&out)==EINVAL);
    old[2].opcode=18; old[2].data_ptr=0; assert(wayne_rpmb_prepare(old,&out)==EINVAL);
    old[2].data_ptr=0x4000; old[2].blocks=1025; assert(wayne_rpmb_prepare(old,&out)==EINVAL);
    old[2].blocks=1; old[2].blksz=4096; assert(wayne_rpmb_prepare(old,&out)==EINVAL);
    puts("RPMB conversion tests passed: 2/3 command preservation, reliable-write bit and invalid requests");
}