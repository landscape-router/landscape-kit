#!/usr/bin/env bash

rustfs_signed_curl() {
  local endpoint=$1
  local access_key=$2
  local secret_key=$3
  local region=$4
  shift 4
  curl --fail-with-body --silent --show-error \
    --aws-sigv4 "aws:amz:${region}:s3" \
    --user "${access_key}:${secret_key}" \
    --header "x-amz-content-sha256: UNSIGNED-PAYLOAD" \
    "$@"
}

rustfs_wait_create_bucket() {
  local endpoint=$1
  local bucket=$2
  local access_key=$3
  local secret_key=$4
  local region=$5
  local attempts=${6:-90}
  local attempt
  for attempt in $(seq 1 "$attempts"); do
    if rustfs_signed_curl "$endpoint" "$access_key" "$secret_key" "$region" \
      --connect-timeout 2 --max-time 5 \
      --request PUT --output /dev/null "$endpoint/$bucket" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

rustfs_write_public_read_policy() {
  local path=$1
  local bucket=$2
  cat >"$path" <<POLICY
{"Version":"2012-10-17","Statement":[{"Sid":"PublicRead","Effect":"Allow","Principal":"*","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::${bucket}/*"]}]}
POLICY
}
