#!/bin/bash

set -e

# Generate the GuestOS configuration.

source /opt/ic/bin/logging.sh
source /opt/ic/bin/metrics.sh

# Get keyword arguments
for argument in "${@}"; do
    case ${argument} in
        -c=* | --config=*)
            CONFIG="${argument#*=}"
            shift
            ;;
        -d=* | --deployment=*)
            DEPLOYMENT="${argument#*=}"
            shift
            ;;
        -h | --help)
            echo 'Usage:
Generate GuestOS Configuration

Arguments:
  -c=, --config=        specify the config.ini configuration file (Default: /boot/config/config.ini)
  -d=, --deployment=    specify the deployment.json configuration file (Default: /boot/config/deployment.json)
  -h, --help            show this help message and exit
  -i=, --input=         specify the input template file (Default: /opt/ic/share/guestos.xml.template)
  -m=, --media=         specify the config media image file (Default: /run/ic-node/config.img)
  -o=, --output=        specify the output configuration file (Default: /var/lib/libvirt/guestos.xml)
'
            exit 1
            ;;
        -i=* | --input=*)
            INPUT="${argument#*=}"
            shift
            ;;
        -m=* | --media=*)
            MEDIA="${argument#*=}"
            shift
            ;;
        -o=* | --output=*)
            OUTPUT="${argument#*=}"
            shift
            ;;
        *)
            echo "Error: Argument is not supported."
            exit 1
            ;;
    esac
done

function validate_arguments() {
    if [ "${CONFIG}" == "" -o "${DEPLOYMENT}" == "" -o "${INPUT}" == "" -o "${OUTPUT}" == "" ]; then
        $0 --help
    fi
}

# Set arguments if undefined
CONFIG="${CONFIG:=/boot/config/config.ini}"
DEPLOYMENT="${DEPLOYMENT:=/boot/config/deployment.json}"
INPUT="${INPUT:=/opt/ic/share/guestos.xml.template}"
MEDIA="${MEDIA:=/run/ic-node/config.img}"
OUTPUT="${OUTPUT:=/var/lib/libvirt/guestos.xml}"

function read_variables() {
    # Read limited set of keys. Be extra-careful quoting values as it could
    # otherwise lead to executing arbitrary shell code!
    while IFS="=" read -r key value; do
        case "$key" in
            "ipv6_prefix") ipv6_prefix="${value}" ;;
            "ipv6_gateway") ipv6_gateway="${value}" ;;
            "ipv4_address") ipv4_address="${value}" ;;
            "ipv4_prefix_length") ipv4_prefix_length="${value}" ;;
            "ipv4_gateway") ipv4_gateway="${value}" ;;
            "domain") domain="${value}" ;;
            "node_reward_type") node_reward_type="${value}" ;;
        esac
    done <"${CONFIG}"
}

function assemble_config_media() {
    cmd=(/opt/ic/bin/build-bootstrap-config-image.sh ${MEDIA})
    cmd+=(--nns_public_key "/boot/config/nns_public_key.pem")
    cmd+=(--ipv6_address "$(/opt/ic/bin/hostos_tool generate-ipv6-address --node-type GuestOS)")
    cmd+=(--ipv6_gateway "${ipv6_gateway}")
    if [[ -n "$ipv4_address" && -n "$ipv4_prefix_length" && -n "$ipv4_gateway" && -n "$domain" ]]; then
        cmd+=(--ipv4_address "${ipv4_address}/${ipv4_prefix_length}")
        cmd+=(--ipv4_gateway "${ipv4_gateway}")
        cmd+=(--domain "${domain}")
    fi
    if [[ -n "$node_reward_type" ]]; then
        cmd+=(--node_reward_type "${node_reward_type}")
    fi
    cmd+=(--hostname "guest-$(/opt/ic/bin/hostos_tool fetch-mac-address | sed 's/://g')")
    cmd+=(--nns_urls "$(/opt/ic/bin/fetch-property.sh --key=.nns.url --metric=hostos_nns_url --config=${DEPLOYMENT})")
    if [ -f "/boot/config/node_operator_private_key.pem" ]; then
        cmd+=(--node_operator_private_key "/boot/config/node_operator_private_key.pem")
    fi

    # Run the above command
    "${cmd[@]}"
    write_log "Assembling config media for GuestOS: ${MEDIA}"
}

function generate_guestos_config() {
    RESOURCES_MEMORY=$(/opt/ic/bin/fetch-property.sh --key=.resources.memory --metric=hostos_resources_memory --config=${DEPLOYMENT})
    MAC_ADDRESS=$(/opt/ic/bin/hostos_tool generate-mac-address --node-type GuestOS)
    RESOURCES_NR_OF_VCPUS="$(jq -r ".resources.nr_of_vcpus" ${DEPLOYMENT})"
    # NOTE: `fetch-property` will error if the target is not found. Here we
    # only want to act when the field is set.
    CPU_MODE=$(jq -r ".resources.cpu" ${DEPLOYMENT})

    # Generate inline CPU spec based on mode
    CPU_SPEC=$(mktemp)
    if [ "${CPU_MODE}" == "qemu" ]; then
        CPU_DOMAIN="qemu"
        cat >"${CPU_SPEC}" <<EOF
<cpu mode='host-model'/>
EOF
    else
        CPU_DOMAIN="kvm"
        CORE_COUNT=$((RESOURCES_NR_OF_VCPUS / 4))
        cat >"${CPU_SPEC}" <<EOF
<cpu mode='host-passthrough' migratable='off'>
  <cache mode='passthrough'/>
  <topology sockets='2' cores='${CORE_COUNT}' threads='2'/>
  <feature policy="require" name="topoext"/>
</cpu>
EOF
    fi

    if [ ! -f "${OUTPUT}" ]; then
        mkdir -p "$(dirname "$OUTPUT")"
        sed -e "s@{{ resources_memory }}@${RESOURCES_MEMORY}@" \
            -e "s@{{ nr_of_vcpus }}@${RESOURCES_NR_OF_VCPUS:-64}@" \
            -e "s@{{ mac_address }}@${MAC_ADDRESS}@" \
            -e "s@{{ cpu_domain }}@${CPU_DOMAIN}@" \
            -e "/{{ cpu_spec }}/{r ${CPU_SPEC}" -e "d" -e "}" \
            "${INPUT}" >"${OUTPUT}"
        restorecon -R "$(dirname "$OUTPUT")"
        write_log "Generating GuestOS configuration file: ${OUTPUT}"
        write_metric "hostos_generate_guestos_config" \
            "1" \
            "HostOS generate GuestOS config" \
            "gauge"
    else
        write_log "GuestOS configuration file already exists: ${OUTPUT}"
        write_metric "hostos_generate_guestos_config" \
            "0" \
            "HostOS generate GuestOS config" \
            "gauge"
    fi

    rm -f "${CPU_SPEC}"
}

function main() {
    # Establish run order
    validate_arguments
    read_variables
    assemble_config_media
    generate_guestos_config
}

main
