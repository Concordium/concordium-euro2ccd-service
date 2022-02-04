# The ubuntu version to build the package in. This influences the dependencies
# that will be added to the package. This should be the same as was used to
# build the binaries.
ARG ubuntu_version

# Build the binary
FROM rust:1.53 as builder

WORKDIR /build

COPY . /build

RUN rustup component add rustfmt

RUN cargo build --release

FROM ubuntu:$ubuntu_version

WORKDIR /build

COPY --from=builder /build/target/release/concordium-eur2ccd /build/concordium-eur2ccd

COPY ./scripts/debian-package/build.sh /build/build.sh
COPY ./resources /build/resources

RUN apt-get update && \
DEBIAN_FRONTEND=noninteractive apt-get -y install debhelper dh-exec

ENV binary=/build/concordium-eur2ccd

RUN /build/build.sh