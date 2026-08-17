ARG DEBIAN_IMAGE=debian:13-slim@sha256:3a39a0592364683e6bab97937b72cad5a8fa6dcbbee90edb3bb48c7f8e94f258

FROM ${DEBIAN_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive

# Offline install: packages were fetched on the host by scripts/flare/fetch-packages.sh.
# Pre-Depends ordering is resolved by retrying unpack/configure a few times;
# no network or apt index is used. The image tracks the host Debian release
# (trixie), so the fetched .deb set matches the base image's glibc.
COPY scripts/flare/packages/*.deb /opt/pkgs/
RUN (for i in 1 2 3 4; do \
       dpkg -i /opt/pkgs/*.deb >/dev/null 2>&1 && break; \
       dpkg --configure -a >/dev/null 2>&1 || true; \
     done; dpkg -i /opt/pkgs/*.deb) \
 && rm -rf /opt/pkgs

COPY scripts/flare/fake_service.py /opt/fake_service.py
COPY scripts/flare/replay_inject.py /opt/replay_inject.py
COPY scripts/flare/rate_flood.py /opt/rate_flood.py
COPY scripts/flare/auth_req_flood.py /opt/auth_req_flood.py
