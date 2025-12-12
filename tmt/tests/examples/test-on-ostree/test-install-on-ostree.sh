# number: 50
# tmt:
#   summary: Test bootc install on ostree env
#   duration: 30m
#   adjust:
#     - when: running_env != existing-ostree
#       enabled: false
#       because: this needs to start an ostree OS firstly
#
#!/bin/bash
set -eux

echo "Testing bootc install on stree"

bootc_img=localhost/bootc-test:latest
bootc_tar=examples/bootc-test.tar
key=root/.ssh/authorized_keys.d/ignition

if [ "$TMT_REBOOT_COUNT" -eq 0 ]; then
    echo "Running before first reboot"
    pwd
    ls -l
    [ -f "${bootc_tar}" ]
    podman load -q -i "${bootc_tar}"
    podman image exists ${bootc_img}
    stateroot=$(bootc status --json | jq -r .status.booted.ostree.stateroot)
    # Run bootc install using the same stateroot for shared /var
    podman run --rm --privileged \
        -v /dev:/dev \
        -v /var/lib/containers:/var/lib/containers \
        -v /:/target \
        --pid=host \
        --security-opt label=type:unconfined_t \
        ${bootc_img} \
            env BOOTC_BOOTLOADER_DEBUG=1 bootc install to-existing-root \
            --stateroot=${stateroot} \
            --skip-fetch-check \
            --acknowledge-destructive \
            --root-ssh-authorized-keys /target/${key} \
            --karg=console=ttyS0,115200n8

    bootc status
    tmt-reboot
elif [ "$TMT_REBOOT_COUNT" -eq 1 ]; then
    echo 'After the reboot'
    booted=$(bootc status --json | jq -r .status.booted.image.image.image)
    [ ${booted} == ${bootc_img} ]
fi

echo "Run bootc install on existing ostree successfully"
