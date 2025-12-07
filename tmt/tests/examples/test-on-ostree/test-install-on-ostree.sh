# number: 26
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

bootc_img=localhost/bootc:latest
bootc_tar=examples/bootc.tar

if [ "$TMT_REBOOT_COUNT" -eq 0 ]; then
    echo "Running before first reboot"
    pwd
    ls -l
    [ -f "${bootc_tar}" ]
    podman load -q -i "${bootc_tar}"
    podman image exists ${bootc_img}
    # Run bootc install
    podman run --rm --privileged -v /dev:/dev -v /var/lib/containers:/var/lib/containers -v /:/target \
    --pid=host --security-opt label=type:unconfined_t \
    ${bootc_img} \
        env BOOTC_BOOTLOADER_DEBUG=1 bootc install to-existing-root \
        --skip-fetch-check \
        --acknowledge-destructive \
        --root-ssh-authorized-keys /target/root/.ssh/authorized_keys.d/ignition \
        --karg=console=ttyS0,115200n8
    
    image=$(bootc status --json |jq -r ".status.otherDeployments[0].image.image.image")
    [ ${image} == ${bootc_img} ]

    tmt-reboot -c "systemctl reboot"
elif [ "$TMT_REBOOT_COUNT" -eq 1 ]; then
    echo 'After the reboot'
    bootc status
fi

echo "Run bootc install on ostree env successfully"
