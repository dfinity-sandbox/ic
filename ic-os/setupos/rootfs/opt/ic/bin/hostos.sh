#!/usr/bin/env bash

set -o nounset
set -o pipefail

SHELL="/bin/bash"
PATH="/sbin:/bin:/usr/sbin:/usr/bin"

function install_hostos() {
    echo "* Installing HostOS disk-image..."

    target_drive=$(find_first_drive)

    size=$(tar --list -v -f /data/host-os.img.tar.gz disk.img | cut -d ' ' -f 3)
    size="${size:=0}"

    tar xzOf /data/host-os.img.tar.gz disk.img | pv -f -s "$size" | dd of="/dev/${target_drive}" bs=10M
    log_and_reboot_on_error "${?}" "Unable to install HostOS disk-image on drive: /dev/${target_drive}"

    sync
    log_and_reboot_on_error "${?}" "Unable to synchronize cached writes to persistent storage."
}

function configure_efi() {
    echo "* Configuring EFI..."

    target_drive=$(find_first_drive)
    partition_prefix=""
    if [[ "${target_drive}" = nvme* ]]; then
        partition_prefix="p"
    fi

    bootnum=$(efibootmgr --verbose | grep "IC-OS" | sed 's/Boot\([0-9A-F]*\).*/\1/')
    for b in ${bootnum}; do
        efibootmgr --delete-bootnum --bootnum ${b} >/dev/null 2>&1
        log_and_reboot_on_error "${?}" "Unable to delete existing 'IC-OS' boot entry."
    done

    efibootmgr --create --gpt --disk "/dev/${target_drive}${partition_prefix}1" --loader "\EFI\BOOT\BOOTX64.EFI" --label "IC-OS" >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to create 'IC-OS' boot entry."

    efibootmgr --remove-dups >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to remove duplicate boot order entries."

    efibootmgr --verbose | grep "IC-OS" | efibootmgr -o $(sed 's/Boot\([0-9A-F]*\).*/\1/') >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to set EFI boot order."
}

function resize_partition() {
    echo "* Resizing partition..."

    target_drive=$(find_first_drive)

    # Repair header at end of disk
    sgdisk --move-second-header "/dev/${target_drive}" >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to extend GPT data structures: /dev/${target_drive}"

    # Extend the LVM partition to fill disk
    parted -s --align optimal "/dev/${target_drive}" "resizepart 3 100%" >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to resize partition: /dev/${target_drive}${partition_prefix}3"

    # Check and update PVs
    pvscan >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable scan physical volumes."

    # Extend PV to the end of LVM partition
    pvresize "/dev/${target_drive}${partition_prefix}3" >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to resize physical volume: /dev/${target_drive}${partition_prefix}3"

    # Check and update VGs
    vgscan >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable scan volume groups."

    # Check and update LVs
    lvscan >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable scan logical volumes."

    # Add additional PVs to VG
    count=0
    large_drives=($(lsblk -nld -o NAME,SIZE | grep 'T$' | grep -o '^\S*'))
    for drive in $(echo ${large_drives[@]}); do
        count=$((count + 1))
        # Avoid adding PV of main disk
        if [ "/dev/${drive}" == "/dev/${target_drive}" ]; then
            continue
        fi

        vgextend hostlvm "/dev/${drive}"
        log_and_reboot_on_error "${?}" "Unable to include PV '/dev/${drive}' in VG."
    done

    # Extend GuestOS LV to fill VG space
    lvextend -i "${count}" --type striped -l +100%FREE /dev/hostlvm/guestos >/dev/null 2>&1
    log_and_reboot_on_error "${?}" "Unable to extend logical volume: /dev/hostlvm/guestos"
}

# Establish run order
main() {
    source /opt/ic/bin/functions.sh
    log_start "$(basename $0)"
    install_hostos
    configure_efi
    resize_partition
    log_end "$(basename $0)"
}

main
