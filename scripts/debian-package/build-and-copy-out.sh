#!/bin/bash

docker build -t ccd-service-builder --build-arg ubuntu_version=20.04 -f scripts/debian-package/deb.Dockerfile .

set -euxo pipefail

id=$(docker create ccd-service-builder)
docker cp $id:/build/pkg-root/ eur2ccd-deb
docker rm $id

# the package is now inside the eur2ccd-deb directory
