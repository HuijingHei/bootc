# NAME

bootc-install-to-disk - Install to the target block device

# SYNOPSIS

**bootc-install-to-disk** \[**\--wipe**\] \[**\--block-setup**\]
\[**\--filesystem**\] \[**\--root-size**\] \[**\--target-transport**\]
\[**\--target-imgref**\] \[**\--target-no-signature-verification**\]
\[**\--target-ostree-remote**\] \[**\--skip-fetch-check**\]
\[**\--disable-selinux**\] \[**\--karg**\] \[**\--generic-image**\]
\[**-h**\|**\--help**\] \[**-V**\|**\--version**\] \<*DEVICE*\>

# DESCRIPTION

Install to the target block device

# OPTIONS

**\--wipe**

:   Automatically wipe all existing data on device

**\--block-setup**=*BLOCK_SETUP* \[default: direct\]

:   Target root block device setup.

direct: Filesystem written directly to block device tpm2-luks: Bind
unlock of filesystem to presence of the default tpm2 device.\

\
\[*possible values: *direct, tpm2-luks\]

**\--filesystem**=*FILESYSTEM*

:   Target root filesystem type\

\
\[*possible values: *xfs, ext4, btrfs\]

**\--root-size**=*ROOT_SIZE*

:   Size of the root partition (default specifier: M). Allowed
    specifiers: M (mebibytes), G (gibibytes), T (tebibytes).

By default, all remaining space on the disk will be used.

**\--target-transport**=*TARGET_TRANSPORT* \[default: registry\]

:   The transport; e.g. oci, oci-archive. Defaults to \`registry\`

**\--target-imgref**=*TARGET_IMGREF*

:   Specify the image to fetch for subsequent updates

**\--target-no-signature-verification**

:   Explicitly opt-out of requiring any form of signature verification

**\--target-ostree-remote**=*TARGET_OSTREE_REMOTE*

:   Enable verification via an ostree remote

**\--skip-fetch-check**

:   By default, the accessiblity of the target image will be verified
    (just the manifest will be fetched). Specifying this option
    suppresses the check; use this when you know the issues it might
    find are addressed.

Two main reasons this might fail:

\- Forgetting \`\--target-no-signature-verification\` if needed - Using
a registry which requires authentication, but not embedding the pull
secret in the image.

**\--disable-selinux**

:   Disable SELinux in the target (installed) system.

This is currently necessary to install \*from\* a system with SELinux
disabled but where the target does have SELinux enabled.

**\--karg**=*KARG*

:   Add a kernel argument

**\--generic-image**

:   Perform configuration changes suitable for a \"generic\" disk image.
    At the moment:

\- All bootloader types will be installed - Changes to the system
firmware will be skipped

**-h**, **\--help**

:   Print help (see a summary with -h)

**-V**, **\--version**

:   Print version

\<*DEVICE*\>

:   Target block device for installation. The entire device will be
    wiped

# VERSION

v0.1.0