#!/usr/bin/env sh
set -eu

ACTION="${1:-}"
AGENT_VERSION="0.1.0"
AGENT_PROTOCOL_VERSION="1"
DEPLOY_ROOT="${DEPLOY_ROOT:-/opt/data/deploy/inxvision-edge}"
DATA_ROOT="${DATA_ROOT:-/opt/data}"
RELEASE_VERSION="${RELEASE_VERSION:-}"
RELEASE_FINGERPRINT="${RELEASE_FINGERPRINT:-}"
REMOTE_PACKAGE="${REMOTE_PACKAGE:-}"
REMOTE_ENV="${REMOTE_ENV:-}"
REMOTE_HOST_INFO="${REMOTE_HOST_INFO:-}"
REMOTE_IMAGE="${REMOTE_IMAGE:-}"
SERVICE_NAME="${SERVICE_NAME:-}"
SERVICE_IMAGE="${SERVICE_IMAGE:-}"
SERVICE_IMAGE_ENV="${SERVICE_IMAGE_ENV:-}"
TASK_ID="${TASK_ID:-}"
MIN_FREE_MB="${MIN_FREE_MB:-1024}"
ALLOW_EXISTING_PORTS="${ALLOW_EXISTING_PORTS:-false}"
PORTS="${PORTS:-1883 6001 6002 7000}"
COMPOSE_CMD="${COMPOSE_CMD:-}"
PLATFORM_API_HOST="${PLATFORM_API_HOST:-}"
PLATFORM_API_PORT="${PLATFORM_API_PORT:-}"
PLATFORM_MQTT_HOST="${PLATFORM_MQTT_HOST:-}"
PLATFORM_MQTT_PORT="${PLATFORM_MQTT_PORT:-}"

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

event() {
  step="$1"
  status="$2"
  message="${3:-}"
  printf '{"step":"%s","status":"%s","message":"%s","time":"%s"}\n' \
    "$(json_escape "$step")" \
    "$(json_escape "$status")" \
    "$(json_escape "$message")" \
    "$(date -Iseconds)"
}

fail() {
  event "$1" "failed" "$2"
  exit "${3:-1}"
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "precheck" "$1 not found" "$2"
}

detect_compose() {
  if [ -n "${COMPOSE_CMD:-}" ]; then
    if $COMPOSE_CMD version >/dev/null 2>&1; then
      return 0
    fi
    fail "precheck" "compose command is not available: ${COMPOSE_CMD}" 11
  fi
  if command -v docker-compose >/dev/null 2>&1; then
    COMPOSE_CMD="docker-compose"
    return 0
  fi
  if docker compose version >/dev/null 2>&1; then
    COMPOSE_CMD="docker compose"
    return 0
  fi
  fail "precheck" "docker-compose or docker compose not found" 11
}

compose() {
  detect_compose
  $COMPOSE_CMD "$@"
}

ensure_data_root() {
  [ -n "$DATA_ROOT" ] || fail "precheck" "DATA_ROOT is empty" 14
  [ "$DATA_ROOT" != "/" ] || fail "precheck" "DATA_ROOT cannot be /" 14
  if [ ! -d "$DATA_ROOT" ]; then
    mkdir -p "$DATA_ROOT" || fail "precheck" "create ${DATA_ROOT} failed" 14
    event "precheck" "running" "created ${DATA_ROOT}"
  fi
  [ -w "$DATA_ROOT" ] || fail "precheck" "${DATA_ROOT} is not writable" 14
}

free_mb() {
  df -Pm "$DATA_ROOT" | awk 'NR==2 {print $4}'
}

check_ports() {
  if [ "$ALLOW_EXISTING_PORTS" = "true" ]; then
    return 0
  fi
  if command -v ss >/dev/null 2>&1; then
    for port in $PORTS; do
      if ss -lnt | awk '{print $4}' | grep -Eq "[:.]${port}$"; then
        fail "precheck" "port ${port} is already in use" 20
      fi
    done
  fi
}

check_tcp_endpoint() {
  host="$1"
  port="$2"
  label="$3"
  [ -n "$host" ] || fail "precheck" "${label}地址未配置" 21
  case "$port" in
    ''|*[!0-9]*) fail "precheck" "${label}端口无效：${port}" 21 ;;
  esac
  [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || fail "precheck" "${label}端口无效：${port}" 21

  if command -v nc >/dev/null 2>&1; then
    nc -z -w 5 "$host" "$port" >/dev/null 2>&1 || fail "precheck" "${label}无法连接：${host}:${port}" 22
  elif command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    timeout 5 bash -c 'exec 3<>"/dev/tcp/$1/$2"' _ "$host" "$port" >/dev/null 2>&1 || fail "precheck" "${label}无法连接：${host}:${port}" 22
  else
    fail "precheck" "无法执行${label}连通性检查，缺少 nc 或 bash/timeout" 23
  fi
  event "precheck" "success" "${label}可达：${host}:${port}"
}

check_platform_endpoints() {
  check_tcp_endpoint "$PLATFORM_API_HOST" "$PLATFORM_API_PORT" "平台 API"
  check_tcp_endpoint "$PLATFORM_MQTT_HOST" "$PLATFORM_MQTT_PORT" "平台 MQTT"
}

precheck() {
  event "precheck" "running" "checking runtime"
  need_cmd docker 10
  detect_compose
  need_cmd tar 12
  need_cmd awk 13
  ensure_data_root

  arch="$(uname -m)"
  [ "$arch" = "x86_64" ] || fail "precheck" "unsupported arch ${arch}" 15

  free="$(free_mb)"
  [ "${free:-0}" -ge "$MIN_FREE_MB" ] || fail "precheck" "free space ${free}MB is less than ${MIN_FREE_MB}MB" 16

  check_ports
	check_platform_endpoints
  compose_version="$(compose version 2>&1)"
  event "precheck" "success" "docker=$(docker --version); compose=${compose_version}; freeMB=${free}"
}

prepare_dirs() {
  mkdir -p \
    "$DATA_ROOT/config/emqx" \
    "$DATA_ROOT/device-edge" \
    "$DATA_ROOT/device-edge/logs" \
    "$DATA_ROOT/rule-engine/db" \
    "$DATA_ROOT/rule-engine/logs" \
    "$DATA_ROOT/emqx/data" \
    "$DATA_ROOT/emqx/log" \
    "$DATA_ROOT/backup" \
    "$DEPLOY_ROOT/releases" \
    "$DEPLOY_ROOT/service-upgrades"
  if command -v chown >/dev/null 2>&1; then
    chown -R 1000:1000 "$DATA_ROOT/emqx" 2>/dev/null || true
  fi
}

current_release_dir() {
  if [ -L "$DEPLOY_ROOT/current" ] || [ -d "$DEPLOY_ROOT/current" ]; then
    readlink -f "$DEPLOY_ROOT/current" 2>/dev/null || true
  fi
}

service_image_env_key() {
  if [ -n "$SERVICE_IMAGE_ENV" ]; then
    printf '%s' "$SERVICE_IMAGE_ENV"
    return 0
  fi
  case "$SERVICE_NAME" in
    emqx)
      printf '%s' "EMQX_IMAGE"
      ;;
    device-edge)
      printf '%s' "DEVICE_EDGE_IMAGE"
      ;;
    rule-engine)
      printf '%s' "RULE_ENGINE_IMAGE"
      ;;
    device-edge-web)
      printf '%s' "DEVICE_EDGE_WEB_IMAGE"
      ;;
    *)
      printf '%s' "$SERVICE_NAME" | tr '[:lower:]' '[:upper:]' | sed 's/[-.]/_/g'
      printf '%s' "_IMAGE"
      ;;
  esac
}

service_exists() {
  current_dir="$1"
  (cd "$current_dir" && compose -f docker-compose.yml config --services) | awk -v service="$SERVICE_NAME" '$0 == service { found = 1 } END { exit found ? 0 : 1 }'
}

require_service_context() {
  [ -n "$SERVICE_NAME" ] || fail "$1" "SERVICE_NAME is required" 60
  current_dir="$(current_release_dir)"
  [ -n "$current_dir" ] && [ -d "$current_dir" ] || fail "$1" "current release does not exist" 61
  [ -f "$current_dir/docker-compose.yml" ] || fail "$1" "current docker-compose.yml does not exist" 62
  [ -f "$current_dir/.env" ] || fail "$1" "current .env does not exist" 63
  service_exists "$current_dir" || fail "$1" "service ${SERVICE_NAME} does not exist in current compose" 64
  printf '%s' "$current_dir"
}

env_value() {
  env_file="$1"
  key="$2"
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; found = 1; exit } END { exit found ? 0 : 1 }' "$env_file"
}

replace_env_value() {
  env_file="$1"
  key="$2"
  value="$3"
  tmp_file="${env_file}.tmp.$$"
  awk -v key="$key" -v value="$value" '
    BEGIN { found = 0 }
    $0 ~ "^" key "=" {
      print key "=" value
      found = 1
      next
    }
    { print }
    END {
      if (!found) {
        print key "=" value
      }
    }
  ' "$env_file" > "$tmp_file"
  mv "$tmp_file" "$env_file"
}

service_upgrade_dir() {
  id="$TASK_ID"
  if [ -z "$id" ]; then
    id="$(date '+%Y%m%d-%H%M%S')"
  fi
  printf '%s' "$DEPLOY_ROOT/service-upgrades/$id"
}

find_release_src() {
  extract_dir="$1"
  if [ -f "$extract_dir/docker-compose.yml" ]; then
    printf '%s' "$extract_dir"
    return 0
  fi
  first_child="$(find "$extract_dir" -mindepth 1 -maxdepth 1 -type d | head -n 1 || true)"
  if [ -n "$first_child" ] && [ -f "$first_child/docker-compose.yml" ]; then
    printf '%s' "$first_child"
    return 0
  fi
  return 1
}

stop_current_release() {
  new_release_dir="$1"
  current_dir=""
  if [ -L "$DEPLOY_ROOT/current" ] || [ -d "$DEPLOY_ROOT/current" ]; then
    current_dir="$(readlink -f "$DEPLOY_ROOT/current" 2>/dev/null || true)"
  fi
  if [ -n "$current_dir" ] && [ "$current_dir" != "$new_release_dir" ] && [ -f "$current_dir/docker-compose.yml" ]; then
    event "compose" "running" "stopping current release ${current_dir}"
    (cd "$current_dir" && compose -f docker-compose.yml down --remove-orphans)
  fi
}

install_release() {
  [ -n "$RELEASE_VERSION" ] || fail "install" "RELEASE_VERSION is required" 30
  [ -f "$REMOTE_PACKAGE" ] || fail "install" "REMOTE_PACKAGE does not exist: ${REMOTE_PACKAGE}" 31
  [ -f "$REMOTE_ENV" ] || fail "install" "REMOTE_ENV does not exist: ${REMOTE_ENV}" 32
  [ -f "$REMOTE_HOST_INFO" ] || fail "install" "REMOTE_HOST_INFO does not exist: ${REMOTE_HOST_INFO}" 33

  precheck
  event "install" "running" "installing release ${RELEASE_VERSION}"
  prepare_dirs

  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
  mkdir -p "$tmp_dir/extract"

  case "$REMOTE_PACKAGE" in
    *.tar.gz|*.tgz)
      tar -xzf "$REMOTE_PACKAGE" -C "$tmp_dir/extract"
      ;;
    *.tar)
      tar -xf "$REMOTE_PACKAGE" -C "$tmp_dir/extract"
      ;;
    *)
      fail "install" "unsupported package type: ${REMOTE_PACKAGE}" 34
      ;;
  esac

  release_src="$(find_release_src "$tmp_dir/extract")" || fail "install" "docker-compose.yml not found in package" 35
  if [ -f "$release_src/checksums/sha256.txt" ]; then
    need_cmd sha256sum 36
    (cd "$release_src" && sha256sum -c checksums/sha256.txt)
  fi

  new_release_dir="$DEPLOY_ROOT/releases/$RELEASE_VERSION"
  rm -rf "$new_release_dir"
  mkdir -p "$new_release_dir"
  cp -a "$release_src/." "$new_release_dir/"
  cp "$REMOTE_ENV" "$new_release_dir/.env"
  cp "$REMOTE_HOST_INFO" "$DATA_ROOT/config/host-info.json"
  if [ -n "$RELEASE_FINGERPRINT" ]; then
    printf '%s\n' "$RELEASE_FINGERPRINT" > "$new_release_dir/release-fingerprint.txt"
  fi

  if [ -d "$new_release_dir/images" ]; then
    for image_tar in "$new_release_dir"/images/*.tar; do
      [ -f "$image_tar" ] || continue
      event "load_images" "running" "$image_tar"
      docker load -i "$image_tar"
    done
  fi

  stop_current_release "$new_release_dir"
  event "compose" "running" "docker-compose up -d --force-recreate"
  (cd "$new_release_dir" && compose -f docker-compose.yml up -d --force-recreate --remove-orphans)
  ln -sfn "$new_release_dir" "$DEPLOY_ROOT/current"
  event "install" "success" "$new_release_dir"
}

backup_current() {
  [ -n "$RELEASE_VERSION" ] || fail "backup" "RELEASE_VERSION is required" 40
  backup_dir="$DATA_ROOT/backup/${RELEASE_VERSION}-before-upgrade-$(date '+%Y%m%d-%H%M%S')"
  mkdir -p "$backup_dir"
  if [ -L "$DEPLOY_ROOT/current" ] || [ -d "$DEPLOY_ROOT/current" ]; then
    current_dir="$(readlink -f "$DEPLOY_ROOT/current" || true)"
    if [ -n "$current_dir" ] && [ -d "$current_dir" ]; then
      cp -a "$current_dir/docker-compose.yml" "$backup_dir/" 2>/dev/null || true
      cp -a "$current_dir/.env" "$backup_dir/" 2>/dev/null || true
      cp -a "$current_dir/manifest.json" "$backup_dir/" 2>/dev/null || true
    fi
  fi
  cp -a "$DATA_ROOT/config/host-info.json" "$backup_dir/" 2>/dev/null || true
  find "$DATA_ROOT/device-edge" "$DATA_ROOT/rule-engine" -maxdepth 3 \( -name '*.db' -o -name '*.sqlite' -o -name '*.sqlite3' \) -exec cp -a {} "$backup_dir/" \; 2>/dev/null || true
  event "backup" "success" "$backup_dir"
}

service_check() {
  event "service_check" "running" "checking service ${SERVICE_NAME}"
  current_dir="$(require_service_context "service_check")"
  image_key="$(service_image_env_key)"
  current_image="$(env_value "$current_dir/.env" "$image_key" 2>/dev/null || true)"
  [ -n "$current_image" ] || fail "service_check" "image variable ${image_key} is missing in current .env" 65
  event "service_check" "success" "service=${SERVICE_NAME}; imageKey=${image_key}; currentImage=${current_image}"
}

service_upgrade() {
  [ -f "$REMOTE_IMAGE" ] || fail "service_upgrade" "REMOTE_IMAGE does not exist: ${REMOTE_IMAGE}" 70
  [ -n "$SERVICE_IMAGE" ] || fail "service_upgrade" "SERVICE_IMAGE is required" 71
  prepare_dirs
  current_dir="$(require_service_context "service_upgrade")"
  image_key="$(service_image_env_key)"
  current_image="$(env_value "$current_dir/.env" "$image_key" 2>/dev/null || true)"
  [ -n "$current_image" ] || fail "service_upgrade" "image variable ${image_key} is missing in current .env" 72

  upgrade_dir="$(service_upgrade_dir)"
  backup_dir="$DATA_ROOT/backup/$(basename "$upgrade_dir")-before-service-upgrade"
  mkdir -p "$upgrade_dir" "$backup_dir"
  cp -a "$current_dir/docker-compose.yml" "$backup_dir/" 2>/dev/null || true
  cp -a "$current_dir/.env" "$backup_dir/.env.before" 2>/dev/null || true
  cp -a "$current_dir/manifest.json" "$backup_dir/" 2>/dev/null || true
  cp -a "$REMOTE_IMAGE" "$upgrade_dir/$(basename "$REMOTE_IMAGE")" 2>/dev/null || true

  event "load_image" "running" "$SERVICE_IMAGE"
  docker load -i "$REMOTE_IMAGE"
  docker image inspect "$SERVICE_IMAGE" >/dev/null 2>&1 || fail "load_image" "loaded image does not contain ${SERVICE_IMAGE}" 73

  replace_env_value "$current_dir/.env" "$image_key" "$SERVICE_IMAGE"
  cp -a "$current_dir/.env" "$upgrade_dir/.env.after" 2>/dev/null || true
  {
    printf '{\n'
    printf '  "service": "%s",\n' "$(json_escape "$SERVICE_NAME")"
    printf '  "imageKey": "%s",\n' "$(json_escape "$image_key")"
    printf '  "beforeImage": "%s",\n' "$(json_escape "$current_image")"
    printf '  "afterImage": "%s",\n' "$(json_escape "$SERVICE_IMAGE")"
    printf '  "backupDir": "%s"\n' "$(json_escape "$backup_dir")"
    printf '}\n'
  } > "$upgrade_dir/metadata.json"

  event "compose" "running" "recreating ${SERVICE_NAME}"
  (cd "$current_dir" && compose -f docker-compose.yml up -d --no-deps --force-recreate "$SERVICE_NAME")
  service_health
  event "service_upgrade" "success" "service=${SERVICE_NAME}; image=${SERVICE_IMAGE}; backup=${backup_dir}"
}

service_health() {
  current_dir="$(require_service_context "service_health")"
  image_key="$(service_image_env_key)"
  expected_image="$SERVICE_IMAGE"
  if [ -z "$expected_image" ]; then
    expected_image="$(env_value "$current_dir/.env" "$image_key" 2>/dev/null || true)"
  fi
  container_id="$(cd "$current_dir" && compose -f docker-compose.yml ps -q "$SERVICE_NAME" 2>/dev/null || true)"
  [ -n "$container_id" ] || fail "service_health" "service ${SERVICE_NAME} container is missing" 80
  status="$(docker inspect -f '{{.State.Status}}' "$container_id" 2>/dev/null || true)"
  [ "$status" = "running" ] || fail "service_health" "service ${SERVICE_NAME} status=${status:-missing}" 81
  if [ -n "$expected_image" ]; then
    actual_image="$(docker inspect -f '{{.Config.Image}}' "$container_id" 2>/dev/null || true)"
    [ "$actual_image" = "$expected_image" ] || fail "service_health" "service ${SERVICE_NAME} image mismatch: current=${actual_image}, expected=${expected_image}" 82
  fi
  (cd "$current_dir" && compose -f docker-compose.yml ps "$SERVICE_NAME")
  event "service_health" "success" "service=${SERVICE_NAME}; image=${expected_image}"
}

health() {
  event "health" "running" "checking containers"
  if ! compose -f "$DEPLOY_ROOT/current/docker-compose.yml" ps >/dev/null 2>&1; then
    fail "health" "compose ps failed" 50
  fi
  for container in inx-edge-emqx inx-device-edge inx-rule-engine inx-device-edge-web; do
    status="$(docker inspect -f '{{.State.Status}}' "$container" 2>/dev/null || true)"
    [ "$status" = "running" ] || fail "health" "${container} status=${status:-missing}" 53
  done
  if [ -n "$RELEASE_VERSION" ]; then
    [ -f "$DEPLOY_ROOT/current/manifest.json" ] || fail "health" "manifest.json not found" 51
    current_version="$(awk -F'"' '/"version"[[:space:]]*:/ {print $4; exit}' "$DEPLOY_ROOT/current/manifest.json")"
    [ "$current_version" = "$RELEASE_VERSION" ] || fail "health" "release version mismatch: current=${current_version}, expected=${RELEASE_VERSION}" 52
  fi
  if [ -n "$RELEASE_FINGERPRINT" ]; then
    [ -f "$DEPLOY_ROOT/current/release-fingerprint.txt" ] || fail "health" "release fingerprint not found" 54
    current_fingerprint="$(tr -d '\r\n' < "$DEPLOY_ROOT/current/release-fingerprint.txt")"
    [ "$current_fingerprint" = "$RELEASE_FINGERPRINT" ] || fail "health" "release fingerprint mismatch: current=${current_fingerprint}, expected=${RELEASE_FINGERPRINT}" 55
  fi
  compose -f "$DEPLOY_ROOT/current/docker-compose.yml" ps
  event "health" "success" "compose ps ok"
}

agent_version() {
  printf '{"agentVersion":"%s","protocolVersion":%s}\n' "$AGENT_VERSION" "$AGENT_PROTOCOL_VERSION"
}

case "$ACTION" in
  version)
    agent_version
    ;;
  precheck)
    precheck
    ;;
  install)
    install_release
    ;;
  backup)
    backup_current
    ;;
  health)
    health
    ;;
  service-check)
    service_check
    ;;
  service-upgrade)
    service_upgrade
    ;;
  service-health)
    service_health
    ;;
  *)
    echo "usage: edge-node-agent.sh version|precheck|install|backup|health|service-check|service-upgrade|service-health" >&2
    exit 2
    ;;
esac

