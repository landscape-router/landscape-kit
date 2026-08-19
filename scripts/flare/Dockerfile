ARG DEBIAN_IMAGE=debian:13-slim@sha256:3a39a0592364683e6bab97937b72cad5a8fa6dcbbee90edb3bb48c7f8e94f258

FROM ${DEBIAN_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive

# Dual-mode runtime dependencies:
# - offline (local dev): packages fetched by scripts/flare/fetch-packages.sh
#   (same Debian release as this image) are installed with dpkg, so
#   `docker build` never needs network access;
# - online (CI): without .deb files the apt path installs them directly.
COPY scripts/flare/packages/ /opt/pkgs/
RUN if ls /opt/pkgs/*.deb >/dev/null 2>&1; then \
      # Pre-Depends ordering is resolved by retrying unpack/configure a few
      # times; no network or apt index is used.
      for i in 1 2 3 4; do \
        dpkg -i /opt/pkgs/*.deb >/dev/null 2>&1 && break; \
        dpkg --configure -a >/dev/null 2>&1 || true; \
      done; \
      dpkg -i /opt/pkgs/*.deb >/dev/null; \
    else \
      apt-get update \
      && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
         iproute2 netcat-openbsd python3 \
      && apt-get clean \
      && rm -rf /var/lib/apt/lists/*; \
    fi \
 && rm -rf /opt/pkgs

COPY scripts/flare/fake_service.py /opt/fake_service.py
COPY scripts/flare/replay_inject.py /opt/replay_inject.py
COPY scripts/flare/rate_flood.py /opt/rate_flood.py
COPY scripts/flare/auth_req_flood.py /opt/auth_req_flood.py
COPY scripts/flare/connection_probe.py /opt/connection_probe.py
