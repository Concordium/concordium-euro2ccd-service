#!/bin/bash

set -euxo pipefail

# This script is intended to be run from the root of the repository.

mkdir -p pkg-root/binaries
mkdir -p pkg-root/debian

if [[ ! -z "${binary}" ]]; then
   cp ${binary} pkg-root/binaries
else 
    # build the service
    cargo build --release
    cp ./target/release/concordium-eur2ccd pkg-root/binaries
fi

export build_version=$(./pkg-root/binaries/concordium-eur2ccd --version | cut -d ' ' -f 2)

cat > pkg-root/debian/changelog <<EOF
concordium-eur2ccd ($build_version) unstable; urgency=medium

   * See changelog https://github.com/Concordium/concordium-euro2ccd-service/CHANGELOG.md for upstream changes.
 
 -- Concordium developers <developers@concordium.com>  Wed, 3 Feb 2022 08:15:00 +2000
EOF

cat > pkg-root/debian/concordium-eur2ccd.install<<EOF
binaries/concordium-eur2ccd /usr/bin/
EOF

cat > pkg-root/debian/compat <<'EOF'
12
EOF

cat > pkg-root/debian/concordium-eur2ccd.service <<'EOF'
[Unit]
Description=Concordium EUR to CCD service
After=syslog.target network.target

[Service]
Type=simple
ExecStart=/usr/bin/concordium-eur2ccd
Restart=always
RestartSec=20

# sandboxing
# mount the entire filesystem as read-only (apart from /dev, /proc and /sys)
ProtectSystem=strict
ProtectClock=yes
PrivateDevices=yes
PrivateTmp=yes
ProtectHostname=yes
ProtectHome=yes
PrivateUsers=yes
ProtectControlGroups=yes
ProtectKernelLogs=yes
ProtectKernelModules=yes
ProtectKernelTunables=yes
CapabilityBoundingSet=
LockPersonality=yes
RestrictRealtime=yes
MemoryDenyWriteExecute=yes

Environment=EUR2CCD_SERVICE_NODE=http://127.0.0.1:10000
Environment=EUR2CCD_SERVICE_RPC_TOKEN=rpcadmin
Environment=EUR2CCD_SERVICE_UPDATE_INTERVAL=1800
Environment=EUR2CCD_SERVICE_PULL_INTERVAL=60
Environment=EUR2CCD_SERVICE_PROMETHEUS_PORT=8112
Environment=EUR2CCD_SERVICE_LOG_LEVEL=debug
Environment=EUR2CCD_SERVICE_SECRET_NAMES=secret1-dummy,secret2-dummy
Environment=EUR2CCD_SERVICE_AWS_REGION=eu-central-1
Environment=EUR2CCD_SERVICE_MAX_RATES_SAVED=60
Environment=EUR2CCD_SERVICE_WARNING_INCREASE_THRESHOLD=30
Environment=EUR2CCD_SERVICE_HALT_INCREASE_THRESHOLD=100
Environment=EUR2CCD_SERVICE_WARNING_DECREASE_THRESHOLD=15
Environment=EUR2CCD_SERVICE_HALT_DECREASE_THRESHOLD=50

[Install]
# start the service when reaching multi-user target
WantedBy=multi-user.target
EOF

cat > pkg-root/debian/control <<'EOF'
Source: concordium-eur2ccd
Maintainer: Concordium developers <developers@concordium.com>
Build-Depends: debconf ( >= 1.5.73 ), debhelper ( >= 12 ), dh-exec

Package: concordium-eur2ccd
Section: extra
Priority: optional
Architecture: amd64
Depends: ${shlibs:Depends}, ${misc:Depends}
Description: Concordium EUR to CCD service
EOF

cat > pkg-root/debian/rules <<'EOF'
#!/usr/bin/make -f
%:
	dh $@

# They will be enabled and started automatically when installed.
# Enabled means they will be started on boot.
# To not enable the service on boot add `--no-enable` to `th_installsystemd`.
# To not start the service automatically upon install add `--no-start`.
override_dh_installsystemd:
		dh_installsystemd --name=concordium-eur2ccd --no-start
EOF

cd pkg-root
dpkg-buildpackage -us -uc --build=binary

mv ../concordium-eur2ccd*.deb ./
