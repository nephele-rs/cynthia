# Build the binary
FROM rustlang/rust:nightly as builder

# Copy in the cargo src
WORKDIR /cynthia
COPY ../cynthia/*             /cynthia

# Build
RUN cargo build --example tcp_echo --release

#FROM alpine:3.13
FROM frolvlad/alpine-glibc

RUN echo "https://mirrors.aliyun.com/alpine/v3.13/main/" > /etc/apk/repositories
RUN echo "https://mirrors.aliyun.com/alpine/v3.13/community" >> /etc/apk/repositories

RUN apk add --no-cache \
    ca-certificates \
    gcc; \
    apk add iptables

RUN apk add bash bash-doc bash-completion

WORKDIR /cynthia
COPY --from=builder /cynthia/target/release/examples/tcp_echo  /cynthia/bin/
COPY examples/identity.pfx /cynthia/bin/
RUN chmod +x /cynthia/bin/tcp_echo

USER root
CMD ["/cynthia/bin/tcp_echo"]
