# Oxys Kernel Config
# Based on CachyOS/Zen kernel best practices
# For podman/kernel/oxysos-kernel.config
# Stock gentoo-sources scheduler by default; no out-of-tree scheduler patch.

# ============================================================
# SCHEDULER
# ============================================================
# Stock mainline EEVDF, no BORE patch and no scheduling service by default.
CONFIG_HZ_1000=y                 # 1000Hz tick rate
CONFIG_HZ=1000
CONFIG_PREEMPT=y                 # Full preemption (desktop responsiveness)
CONFIG_PREEMPT_VOLUNTARY=n
CONFIG_PREEMPT_NONE=n

# sched-ext is not enabled in this profile. This was verified against the
# gentoo-sources-7.0.14-r1 tree (CONFIG_SCHED_CLASS_EXT was not exposed
# there, so olddefconfig dropped the symbol if requested). The kernel pin has
# since moved to gentoo-sources-6.18.38 (latest stable amd64) -- re-verify
# CONFIG_SCHED_CLASS_EXT availability against that tree before assuming this
# still holds. Keep this as stock EEVDF until then.

# ============================================================
# ZFS — out-of-tree module, not built in
# ============================================================
# ZFS is CDDL-licensed and is not compiled into the GPLv2 Linux kernel image.
# Oxys ships it through Gentoo's sys-fs/zfs-kmod package, built as a loadable
# module against the exact kernel release produced by this build.
#
# Keep the gentoo-sources pin within the Linux versions supported by the
# selected OpenZFS release. The kernel profile currently pins
# gentoo-sources-6.18.38 (latest stable amd64) with sys-fs/zfs-kmod-2.3.6 and
# sys-fs/zfs-2.3.6 (latest stable amd64; OpenZFS 2.3.6 supports Linux
# 4.18-6.19, which covers 6.18.38). Letting either side float can produce a
# broken pair: newer gentoo-sources may exceed the OpenZFS-supported kernel
# range, and newer ZFS userland can block an older zfs-kmod package.
#
# Kernel config requirements for that path live in podman/kernel/base.config:
# CONFIG_MODULES=y
# CONFIG_MODULE_UNLOAD=y
# CONFIG_MODVERSIONS=y
# CONFIG_MODULE_SRCVERSION_ALL=y
# CONFIG_KALLSYMS=y
# CONFIG_KALLSYMS_ALL=y
# CONFIG_BLK_DEV_INITRD=y
# CONFIG_CRYPTO_DEFLATE=y
# CONFIG_ZLIB_DEFLATE=y
# CONFIG_ZLIB_INFLATE=y
#
# Loading zfs.ko taints the kernel because it is an out-of-tree, non-GPL module.
# The dmesg taint line is expected and cosmetic; it is not by itself a ZFS
# functional failure.

# ============================================================
# FILESYSTEMS — minimal, no cancer
# ============================================================
CONFIG_EXT4_FS=y                 
CONFIG_BTRFS_FS=y
CONFIG_XFS_FS=y                  
CONFIG_TMPFS=y      
CONFIG_PROC_FS=y
CONFIG_SYSFS=y

# ============================================================
# COMPILER OPTIMISATIONS — per arch
# ============================================================
# IMPORTANT: this is a multi-arch build pipeline. Do not use -march=native or
# CONFIG_MNATIVE_* for distributable artifacts; native only targets the CI/build
# CPU. The container build sets the explicit target through OXYS_MARCH instead:
#
#   v3        -> x86-64-v3
#   alderlake -> alderlake
#   znver3    -> znver3
#   znver4    -> znver4
#   znver5    -> znver5
#
# Local one-machine builds may use native, but native-built artifacts should not
# be shipped.

CONFIG_CC_OPTIMIZE_FOR_PERFORMANCE=y
# CONFIG_LTO_CLANG_THIN is intentionally not forced here; the current builder
# is verified without switching the kernel toolchain to clang/LTO.

# ============================================================
# MEMORY
# ============================================================
CONFIG_TRANSPARENT_HUGEPAGE=y
CONFIG_TRANSPARENT_HUGEPAGE_ALWAYS=n
CONFIG_TRANSPARENT_HUGEPAGE_MADVISE=y  # let apps decide
CONFIG_ZSWAP=y
CONFIG_ZRAM=y

# ============================================================
# I/O SCHEDULER
# ============================================================
CONFIG_MQ_IOSCHED_KYBER=y
CONFIG_IOSCHED_BFQ=y
# NVMe gets 'none', SATA gets 'mq-deadline', HDD gets 'bfq'
# (set via udev rules at runtime, not kernel config)

# ============================================================
# NETWORKING
# ============================================================
CONFIG_NET=y
CONFIG_UNIX=y
CONFIG_INET=y
CONFIG_TCP_CONG_BBR=y            # BBRv3 congestion control
CONFIG_NET_SCH_FQ=y              # needed by BBR
CONFIG_IPV6=y
CONFIG_NET_NS=y

# ============================================================
# GPU — Intel iGPU (alderlake) + AMD (znver builds)
# ============================================================
CONFIG_PCI=y                     # required for discrete/integrated PCI GPUs
CONFIG_DRM=y
CONFIG_DRM_I915=y                # Intel (alderlake)
CONFIG_DRM_AMDGPU=y              # AMD (znver3/4/5)
CONFIG_DRM_NOUVEAU=y            
CONFIG_DRM_SIMPLEDRM=y           # fallback

# Wayland needs these
CONFIG_DRM_KMS_HELPER=y
CONFIG_FB=n                      # disable legacy framebuffer

# ============================================================
# POWER MANAGEMENT
# ============================================================
CONFIG_CPU_FREQ=y
CONFIG_CPU_FREQ_GOV_SCHEDUTIL=y  # best for desktop
CONFIG_CPU_FREQ_DEFAULT_GOV_SCHEDUTIL=y
CONFIG_INTEL_IDLE=y
CONFIG_INTEL_PSTATE=y            # Intel P-state driver
CONFIG_X86_AMD_PSTATE=y          # AMD P-state (for znver builds)
CONFIG_X86_AMD_PSTATE_DEFAULT_MODE=3  # active mode

# ============================================================
# SECURITY
# ============================================================
CONFIG_SECURITY=y
CONFIG_SECURITY_LANDLOCK=y
CONFIG_HARDENED_USERCOPY=y
CONFIG_FORTIFY_SOURCE=y
CONFIG_STACKPROTECTOR_STRONG=y
CONFIG_RANDOMIZE_BASE=y          # KASLR

# ============================================================
# DISABLE BLOAT
# ============================================================
CONFIG_STAGING=n                 # staging drivers, not needed
CONFIG_HAMRADIO=n
CONFIG_ISDN=n
CONFIG_PCMCIA=n                  # old laptop cards
CONFIG_CARDBUS=n
CONFIG_MEDIA_SUPPORT=n           # TV tuners etc, not needed
CONFIG_DVB_CORE=n
CONFIG_JOYSTICK=n                # no joysticks (gaming via Wayland handles this)
CONFIG_TABLET_USB_ACECAD=n
CONFIG_SOUND_OSS_CORE=n          # OSS dead, we use PipeWire
CONFIG_SND_OSSEMUL=n

# ============================================================
# BASELINE REQUIREMENTS (Portage sandboxing + modern userspace)
# ============================================================
# These are not systemd-specific. They are required by Portage sandbox/cgroup
# support or by modern userspace components such as udev and PipeWire.
CONFIG_CGROUPS=y
CONFIG_CGROUP_BPF=y
CONFIG_NAMESPACES=y
CONFIG_USER_NS=y
CONFIG_INOTIFY_USER=y
CONFIG_FANOTIFY=y
CONFIG_TMPFS_POSIX_ACL=y
CONFIG_TMPFS_XATTR=y
CONFIG_SECCOMP=y
CONFIG_SECCOMP_FILTER=y
CONFIG_BPF_SYSCALL=y

# ============================================================
# BUILD NOTES
# ============================================================
# 1. Copy this config as .config
# 2. make olddefconfig (fills in any missing options)
# 3. Per arch: set the explicit target through OXYS_MARCH
# 4. make -j20
# 5. make modules_install
# 6. Build sys-fs/zfs-kmod against the same /usr/src/linux tree
# 7. Verify zfs.ko is installed under /lib/modules/$(make kernelrelease)
# Output: vmlinuz + kernel modules + zfs-kmod tarballs per arch

# ============================================================
# KERNEL/ZFS OUTPUT PAIRING
# ============================================================
# The kernel build profile writes one shared build id per architecture under
# output/<arch>/build-id. The id is
# <build-utc>-gentoo-<portage-snapshot-utc>, with both timestamps formatted as
# YYYYMMDDTHHMMSSZ. Kernel and zfs-kmod archives include that id in their
# filenames and each archive gets a .metadata sidecar containing build_id,
# arch, atom, version, and kernel_release.
#
# Example names:
#   kernel-alderlake-7.0.14-gentoo-oxys-20260706T010203Z-gentoo-20260705T123752Z.tar.gz
#   zfs-kmod-alderlake-20260706T010203Z-gentoo-20260705T123752Z-2.3.8.tar.gz
#
# During the kernel build profile, zfs-kmod is validated after emerge by
# checking that zfs.ko exists under the exact /lib/modules/<kernel_release>
# produced earlier in the same run and that modinfo vermagic matches that
# release. The build container can only live-load zfs.ko when the running kernel
# is that same release; otherwise it records the mismatch and skips the live
# load because Linux cannot load a module built for another kernel.
