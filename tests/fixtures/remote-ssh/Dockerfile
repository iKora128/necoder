FROM ubuntu:24.04

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        openssh-server \
        procps \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash dev \
    && passwd -d dev \
    && mkdir -p /run/sshd

COPY sshd_config /etc/ssh/sshd_config
COPY entrypoint.sh /usr/local/bin/shirushi-test-entrypoint
RUN chmod 755 /usr/local/bin/shirushi-test-entrypoint

EXPOSE 22

ENTRYPOINT ["/usr/local/bin/shirushi-test-entrypoint"]
