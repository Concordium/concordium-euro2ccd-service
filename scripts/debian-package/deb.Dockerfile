# The ubuntu version to build the package in. This influences the dependencies
# that will be added to the package. This should be the same as was used to
# build the binaries.
ARG ubuntu_version

# Build the binary
FROM rust:1.62 as builder

# Install protobuf
RUN wget https://github.com/protocolbuffers/protobuf/releases/download/v3.15.3/protoc-3.15.3-linux-x86_64.zip \
    && unzip protoc-3.15.3-linux-x86_64.zip \
    && mv ./bin/protoc /usr/bin/protoc \
    && chmod +x /usr/bin/protoc

WORKDIR /build

COPY . /build

RUN rustup component add rustfmt

RUN cargo build --release

FROM ubuntu:$ubuntu_version

WORKDIR /build

COPY --from=builder /build/target/release/concordium-eur2ccd /build/concordium-eur2ccd

COPY ./scripts/debian-package/build.sh /build/build.sh

RUN apt-get update && \
DEBIAN_FRONTEND=noninteractive apt-get -y install debhelper dh-exec

ENV binary=/build/concordium-eur2ccd

RUN /build/build.sh
