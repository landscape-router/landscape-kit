ARG DEBIAN_IMAGE=debian:12.11-slim@sha256:b1a741487078b369e78119849663d7f1a5341ef2768798f7b7406c4240f86aef

FROM ${DEBIAN_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive

# Offline install: packages were fetched on the host by fetch-packages.sh.
# Pre-Depends ordering is resolved by retrying unpack/configure a few times;
# no network or apt index is used.
COPY packages/*.deb /opt/pkgs/
RUN (for i in 1 2 3 4; do \
       dpkg -i /opt/pkgs/*.deb >/dev/null 2>&1 && break; \
       dpkg --configure -a >/dev/null 2>&1 || true; \
     done; dpkg -i /opt/pkgs/*.deb) \
 && rm -rf /opt/pkgs

COPY fake_service.py /opt/fake_service.py
COPY replay_inject.py /opt/replay_inject.py
COPY rate_flood.py /opt/rate_flood.py
COPY auth_req_flood.py /opt/auth_req_flood.py
