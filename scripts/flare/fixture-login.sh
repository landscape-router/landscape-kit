# Sourced by the Docker scenarios. The expected identity comes from the server
# console, independently of the Ethernet discovery traffic under test.
fixture_login() {
  local server=$1 fingerprint
  docker exec -i "$server" chpasswd <<<"root:fixture-root-password"
  for _ in $(seq 1 45); do
    fingerprint=$(docker exec "$server" /opt/bin/lkit flare setup | sed -n 's/^flare: device fingerprint: //p')
    if [[ $fingerprint =~ ^[0-9a-f]{64}$ ]]; then
      printf '%s\n' "$fingerprint"
      return
    fi
    sleep 1
  done
  echo "fixture device identity unavailable" >&2
  return 1
}
