# The host script verifies this fixed minirootfs archive before Docker sees it.
FROM scratch
ADD alpine-minirootfs.tar.gz /
RUN printf '%s\n' 'https://dl-cdn.alpinelinux.org/alpine/v3.22/main' > /etc/apk/repositories \
    && apk add --no-cache alpine-base linux-virt openssh iproute2 \
    && printf '%s\n' 'features="base ext4 virtio"' > /etc/mkinitfs/mkinitfs.conf \
    && mkinitfs -o /boot/initramfs-virt "$(ls /lib/modules)" \
    && passwd -d root \
    && ssh-keygen -A \
    && rc-update add devfs sysinit \
    && rc-update add sysfs sysinit \
    && rc-update add procfs sysinit \
    && rc-update add modules boot \
    && rc-update add bootmisc boot \
    && rc-update add hostname boot \
    && rc-update add localmount boot \
    && rc-update add networking default \
    && rc-update add sshd default
CMD ["/bin/sh"]
