#!/usr/bin/env sh
set -eu

ACTION="${1:-}"
AGENT_VERSION="0.1.10"
AGENT_PROTOCOL_VERSION="1"
DEPLOY_ROOT="${DEPLOY_ROOT:-/opt/data/deploy}"
DATA_ROOT="${DATA_ROOT:-/opt/data}"
RELEASE_VERSION="${RELEASE_VERSION:-}"
RELEASE_FINGERPRINT="${RELEASE_FINGERPRINT:-}"
REMOTE_PACKAGE="${REMOTE_PACKAGE:-}"
REMOTE_ENV="${REMOTE_ENV:-}"
REMOTE_HOST_INFO="${REMOTE_HOST_INFO:-}"
REMOTE_COMPOSE="${REMOTE_COMPOSE:-}"
REMOTE_IMAGE="${REMOTE_IMAGE:-}"
SERVICE_NAME="${SERVICE_NAME:-}"
SERVICE_IMAGE="${SERVICE_IMAGE:-}"
SERVICE_IMAGE_ENV="${SERVICE_IMAGE_ENV:-}"
TASK_ID="${TASK_ID:-}"
MIN_FREE_MB="${MIN_FREE_MB:-1024}"
BACKUP_RETENTION_DAYS="${BACKUP_RETENTION_DAYS:-30}"
SERVICE_UPGRADE_RETENTION_DAYS="${SERVICE_UPGRADE_RETENTION_DAYS:-14}"
STAGING_RETENTION_DAYS="${STAGING_RETENTION_DAYS:-3}"
ALLOW_EXISTING_PORTS="${ALLOW_EXISTING_PORTS:-false}"
PORTS="${PORTS:-}"
COMPOSE_CMD="${COMPOSE_CMD:-}"
PLATFORM_API_HOST="${PLATFORM_API_HOST:-}"
PLATFORM_API_PORT="${PLATFORM_API_PORT:-}"
PLATFORM_MQTT_HOST="${PLATFORM_MQTT_HOST:-}"
PLATFORM_MQTT_PORT="${PLATFORM_MQTT_PORT:-}"
SERVICE_CHECK_SOURCE="${SERVICE_CHECK_SOURCE:-manual}"
ROLLBACK_OBSERVED="false"
OBSERVATION_RELEASE=""
OBSERVATION_CACHE=""
OBSERVATION_SERVICES=""

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

event() {
  step="$1"
  status="$2"
  message="${3:-}"
  if [ "$step" = "rollback" ]; then
    ROLLBACK_OBSERVED="true"
    if [ "$status" = "running" ]; then reset_runtime_observation ""; fi
  fi
  printf '{"step":"%s","status":"%s","message":"%s","time":"%s"}\n' \
    "$(json_escape "$step")" \
    "$(json_escape "$status")" \
    "$(json_escape "$message")" \
    "$(date -Iseconds)"
}

fail() {
  event "$1" "failed" "$2"
  case "$ACTION" in
    install|service-upgrade|health|service-health)
      (if [ "$ROLLBACK_OBSERVED" = "true" ]; then SERVICE_CHECK_SOURCE="rollback"; fi
       inspect_services) || true
      ;;
  esac
  exit "${3:-1}"
}

require_safe_release_version() {
  [ -n "$RELEASE_VERSION" ] || fail "$1" "RELEASE_VERSION is required" "$2"
  [ "${#RELEASE_VERSION}" -le 128 ] || fail "$1" "RELEASE_VERSION is too long" "$2"
  case "$RELEASE_VERSION" in
    .|..|*[!A-Za-z0-9._-]*)
      fail "$1" "RELEASE_VERSION contains unsafe characters" "$2"
      ;;
  esac
  case "$RELEASE_VERSION" in
    [A-Za-z0-9]*) ;;
    *) fail "$1" "RELEASE_VERSION must start with a letter or number" "$2" ;;
  esac
}

require_safe_root() {
  name="$1"
  value="$2"
  code="$3"
  [ -n "$value" ] || fail "precheck" "${name} is empty" "$code"
  [ "$value" != "/" ] || fail "precheck" "${name} cannot be /" "$code"
  case "$value" in
    /*) ;;
    *) fail "precheck" "${name} must be an absolute path" "$code" ;;
  esac
  case "$value/" in
    *"//"*|*"/./"*|*"/../"*) fail "precheck" "${name} contains unsafe path segments" "$code" ;;
  esac
}

require_retention_days() {
  name="$1"
  value="$2"
  case "$value" in
    ''|*[!0-9]*) fail "retention" "${name} must be an integer number of days" 17 ;;
  esac
  [ "$value" -ge 1 ] && [ "$value" -le 3650 ] || fail "retention" "${name} must be between 1 and 3650 days" 17
}

prune_directory() {
  root="$1"
  days="$2"
  label="$3"
  [ -e "$root" ] || return 0
  [ -d "$root" ] || fail "retention" "${label} root is not a directory: ${root}" 18
  [ ! -L "$root" ] || fail "retention" "${label} root cannot be a symbolic link: ${root}" 18
  event "retention" "running" "pruning ${label} children older than ${days} days"
  if ! find "$root" -mindepth 1 -maxdepth 1 -type d -mtime "+${days}" -exec rm -rf -- {} +; then
    fail "retention" "failed to prune expired ${label} directories" 19
  fi
  event "retention" "success" "${label} retention applied"
}

prune_retention() {
  require_retention_days "BACKUP_RETENTION_DAYS" "$BACKUP_RETENTION_DAYS"
  require_retention_days "SERVICE_UPGRADE_RETENTION_DAYS" "$SERVICE_UPGRADE_RETENTION_DAYS"
  require_retention_days "STAGING_RETENTION_DAYS" "$STAGING_RETENTION_DAYS"
  prune_directory "$DATA_ROOT/backup" "$BACKUP_RETENTION_DAYS" "backup"
  prune_directory "$DEPLOY_ROOT/service-upgrades" "$SERVICE_UPGRADE_RETENTION_DAYS" "service-upgrades"
  prune_directory "$DATA_ROOT/.inxaiot-desk-buddy" "$STAGING_RETENTION_DAYS" "staging"
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
  require_safe_root "DATA_ROOT" "$DATA_ROOT" 14
  require_safe_root "DEPLOY_ROOT" "$DEPLOY_ROOT" 14
  if [ ! -d "$DATA_ROOT" ]; then
    mkdir -p "$DATA_ROOT" || fail "precheck" "create ${DATA_ROOT} failed" 14
    event "precheck" "running" "created ${DATA_ROOT}"
  fi
  [ -w "$DATA_ROOT" ] || fail "precheck" "${DATA_ROOT} is not writable" 14
  if [ ! -d "$DEPLOY_ROOT" ]; then
    mkdir -p "$DEPLOY_ROOT" || fail "precheck" "create ${DEPLOY_ROOT} failed" 14
    event "precheck" "running" "created ${DEPLOY_ROOT}"
  fi
  [ -w "$DEPLOY_ROOT" ] || fail "precheck" "${DEPLOY_ROOT} is not writable" 14
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
  prune_retention

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
    "$DATA_ROOT/.inxaiot-desk-buddy" \
    "$DEPLOY_ROOT/releases" \
    "$DEPLOY_ROOT/service-upgrades"
  if command -v chown >/dev/null 2>&1; then
    chown -R 1000:1000 "$DATA_ROOT/emqx" 2>/dev/null || true
  fi
  prune_retention
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
  fail "service_config" "SERVICE_IMAGE_ENV is required for Compose-driven service upgrade" 66
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

load_image_archive_with_tag() {
  image_tar="$1"
  expected_image="$2"
  [ -f "$image_tar" ] || return 1
  [ -n "$expected_image" ] || return 1
  if ! load_output="$(docker load -i "$image_tar" 2>&1)"; then
    printf '%s\n' "$load_output" >&2
    return 1
  fi
  printf '%s\n' "$load_output"
  loaded_id="$(printf '%s\n' "$load_output" | awk '
    /^Loaded image ID:[[:space:]]*/ {
      current = $0
      sub(/^Loaded image ID:[[:space:]]*/, "", current)
      if (found && current != value) ambiguous = 1
      value = current
      found = 1
    }
    END {
      if (ambiguous) exit 1
      if (found) print value
    }
  ')" || return 1
  if [ -n "$loaded_id" ]; then
    printf '%s' "$loaded_id" | grep -Eq '^sha256:[0-9a-f]{64}$' || return 1
    docker image tag "$loaded_id" "$expected_image" || return 1
  fi
  docker image inspect "$expected_image" >/dev/null 2>&1
}

find_release_src() {
  extract_dir="$1"
  if [ -f "$extract_dir/manifest.json" ]; then
    printf '%s' "$extract_dir"
    return 0
  fi
  first_child="$(find "$extract_dir" -mindepth 1 -maxdepth 1 -type d | head -n 1 || true)"
  if [ -n "$first_child" ] && [ -f "$first_child/manifest.json" ]; then
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

verify_release_dir() {
  release_dir="$1"
  (cd "$release_dir" && compose -f docker-compose.yml ps >/dev/null 2>&1) || return 1
  verify_runtime_containers "$release_dir" || return 1
  [ -f "$release_dir/manifest.json" ] || return 1
  current_version="$(awk -F'"' '/"version"[[:space:]]*:/ {print $4; exit}' "$release_dir/manifest.json")"
  [ "$current_version" = "$RELEASE_VERSION" ] || return 1
  if [ -n "$RELEASE_FINGERPRINT" ]; then
    [ -f "$release_dir/release-fingerprint.txt" ] || return 1
    current_fingerprint="$(tr -d '\r\n' < "$release_dir/release-fingerprint.txt")"
    [ "$current_fingerprint" = "$RELEASE_FINGERPRINT" ] || return 1
  fi
}

wait_for_release() {
  release_dir="$1"
  attempt=0
  while [ "$attempt" -lt 30 ]; do
    if verify_release_dir "$release_dir"; then
      return 0
    fi
    attempt=$((attempt + 1))
    if [ $((attempt % 5)) -eq 0 ]; then
      event "health_wait" "running" "waiting for Compose services (${attempt}/30)"
    fi
    sleep 2
  done
  return 1
}

verify_runtime_containers() {
  release_dir="$1"
  reset_runtime_observation "$release_dir"
  services="$(cd "$release_dir" && compose -f docker-compose.yml config --services 2>/dev/null || true)"
  OBSERVATION_SERVICES="$services"
  [ -n "$services" ] || return 1
  for service in $services; do
    observe_runtime_service "$release_dir" "$service" "verification"
    [ "$OBSERVATION_RESULT" = "ok" ] || return 1
    status="$(printf '%s\n' "$OBSERVATION_FACTS" | awk -F'|' '{print $1}')"
    [ "$status" = "running" ] || return 1
  done
}

rollback_release() {
  new_release_dir="$1"
  previous_release="$2"
  host_info_backup="$3"
  had_host_info="$4"
  event "rollback" "running" "restoring previous release"
  if [ -f "$new_release_dir/docker-compose.yml" ]; then
    (cd "$new_release_dir" && compose -f docker-compose.yml down --remove-orphans) >/dev/null 2>&1 || true
  fi
  if [ -n "$previous_release" ] && [ -f "$previous_release/docker-compose.yml" ]; then
    (cd "$previous_release" && compose -f docker-compose.yml up -d --force-recreate --remove-orphans) || return 1
    ln -sfn "$previous_release" "$DEPLOY_ROOT/current"
  else
    rm -f "$DEPLOY_ROOT/current"
  fi
  if [ "$had_host_info" = "true" ]; then
    cp "$host_info_backup" "$DATA_ROOT/config/host-info.json" || return 1
  else
    rm -f "$DATA_ROOT/config/host-info.json"
  fi
  case "$new_release_dir" in
    "$DEPLOY_ROOT"/releases/*) rm -rf "$new_release_dir" ;;
    *) return 1 ;;
  esac
  event "rollback" "success" "previous release restored"
}

install_release() {
  require_safe_release_version "install" 30
  [ -f "$REMOTE_PACKAGE" ] || fail "install" "REMOTE_PACKAGE does not exist: ${REMOTE_PACKAGE}" 31
  [ -f "$REMOTE_ENV" ] || fail "install" "REMOTE_ENV does not exist: ${REMOTE_ENV}" 32
  [ -f "$REMOTE_HOST_INFO" ] || fail "install" "REMOTE_HOST_INFO does not exist: ${REMOTE_HOST_INFO}" 33
  [ -f "$REMOTE_COMPOSE" ] || fail "install" "REMOTE_COMPOSE does not exist: ${REMOTE_COMPOSE}" 34

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

  release_src="$(find_release_src "$tmp_dir/extract")" || fail "install" "internal manifest.json not found in package" 35
  if [ -f "$release_src/checksums/sha256.txt" ]; then
    need_cmd sha256sum 36
    (cd "$release_src" && sha256sum -c checksums/sha256.txt)
  fi

  previous_release="$(current_release_dir)"
  new_release_dir="$DEPLOY_ROOT/releases/$RELEASE_VERSION"
  [ ! -e "$new_release_dir" ] || fail "install" "release version already exists and is immutable: ${RELEASE_VERSION}" 37
  host_info_backup="$tmp_dir/host-info.before"
  had_host_info="false"
  if [ -f "$DATA_ROOT/config/host-info.json" ]; then
    cp "$DATA_ROOT/config/host-info.json" "$host_info_backup"
    had_host_info="true"
  fi
  mkdir -p "$new_release_dir"
  cp -a "$release_src/." "$new_release_dir/"
  cp "$REMOTE_ENV" "$new_release_dir/.env"
  cp "$REMOTE_COMPOSE" "$new_release_dir/docker-compose.yml"
  cp "$REMOTE_HOST_INFO" "$DATA_ROOT/config/host-info.json"
  if [ -n "$RELEASE_FINGERPRINT" ]; then
    printf '%s\n' "$RELEASE_FINGERPRINT" > "$new_release_dir/release-fingerprint.txt"
  fi

  if [ -d "$new_release_dir/images" ]; then
    for image_tar in "$new_release_dir"/images/*.tar; do
      [ -f "$image_tar" ] || continue
      tag_file="${image_tar%.tar}.tag"
      [ -f "$tag_file" ] || fail "load_images" "image tag metadata is missing: ${tag_file}" 36
      expected_image="$(tr -d '\r\n' < "$tag_file")"
      event "load_images" "running" "$image_tar"
      load_image_archive_with_tag "$image_tar" "$expected_image" ||
        fail "load_images" "loaded image cannot be assigned expected tag: ${expected_image}" 36
    done
  fi

  stop_current_release "$new_release_dir"
  event "compose" "running" "docker-compose up -d --force-recreate"
  if ! (cd "$new_release_dir" && compose -f docker-compose.yml up -d --force-recreate --remove-orphans); then
    if ! rollback_release "$new_release_dir" "$previous_release" "$host_info_backup" "$had_host_info"; then
      fail "rollback" "new release failed and previous release rollback failed" 38
    fi
    fail "install" "new release compose start failed; previous release restored" 39
  fi
  ln -sfn "$new_release_dir" "$DEPLOY_ROOT/current"
  if ! wait_for_release "$new_release_dir"; then
    if ! rollback_release "$new_release_dir" "$previous_release" "$host_info_backup" "$had_host_info"; then
      fail "rollback" "new release health failed and previous release rollback failed" 40
    fi
    fail "install" "new release health failed; previous release restored" 41
  fi
  event "install" "success" "$new_release_dir"
}

backup_current() {
  require_safe_release_version "backup" 40
  ensure_data_root
  prepare_dirs
  current_dir="$(current_release_dir)"
  [ -n "$current_dir" ] && [ -d "$current_dir" ] || fail "backup" "current release does not exist" 41
  for required in docker-compose.yml .env manifest.json; do
    [ -f "$current_dir/$required" ] || fail "backup" "required backup file is missing: $required" 42
  done
  [ -f "$DATA_ROOT/config/host-info.json" ] || fail "backup" "host-info.json is missing" 44

  backup_dir="$DATA_ROOT/backup/${RELEASE_VERSION}-before-upgrade-$(date '+%Y%m%d-%H%M%S')"
  mkdir "$backup_dir" || fail "backup" "failed to create unique backup directory" 43
  for required in docker-compose.yml .env manifest.json; do
    cp -a "$current_dir/$required" "$backup_dir/" || fail "backup" "failed to copy $required" 43
  done
  cp -a "$DATA_ROOT/config/host-info.json" "$backup_dir/" || fail "backup" "failed to copy host-info.json" 45
  database_file_list="$backup_dir/.database-files.list"
  for data_dir in "$DATA_ROOT/device-edge" "$DATA_ROOT/rule-engine"; do
    if [ -d "$data_dir" ]; then
      find "$data_dir" -maxdepth 3 -type f \( -name '*.db' -o -name '*.sqlite' -o -name '*.sqlite3' \) -print >> "$database_file_list" || fail "backup" "failed to enumerate service database files" 46
    fi
  done
  if [ -s "$database_file_list" ]; then
    event "compose" "running" "stopping current release for consistent database backup"
    if ! (cd "$current_dir" && compose -f docker-compose.yml stop); then
      if ! (cd "$current_dir" && compose -f docker-compose.yml up -d); then
        fail "backup" "compose stop failed and service recovery also failed" 47
      fi
      rm -rf -- "$backup_dir"
      fail "backup" "compose stop failed; services were restored" 46
    fi

    copy_failed="false"
    while IFS= read -r database_file; do
      [ -n "$database_file" ] || continue
      relative_path="${database_file#"$DATA_ROOT"/}"
      target_file="$backup_dir/data/$relative_path"
      if ! mkdir -p "$(dirname "$target_file")" || ! cp -a "$database_file" "$target_file"; then
        copy_failed="true"
        break
      fi
    done < "$database_file_list"

    event "compose" "running" "restarting current release after database backup"
    if ! (cd "$current_dir" && compose -f docker-compose.yml up -d); then
      rm -f "$database_file_list"
      fail "backup" "database backup finished but current release restart failed" 48
    fi
    if ! verify_runtime_containers "$current_dir"; then
      rm -f "$database_file_list"
      fail "backup" "current release restart did not restore all expected containers" 49
    fi
    if [ "$copy_failed" = "true" ]; then
      rm -rf -- "$backup_dir"
      fail "backup" "database copy failed; services were restored and partial backup removed" 46
    fi
  fi
  rm -f "$database_file_list"
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

verify_service_state() {
  service_dir="$1"
  expected_image="$2"
  reset_runtime_observation "$service_dir"
  observe_runtime_service "$service_dir" "$SERVICE_NAME" "verification"
  [ "$OBSERVATION_RESULT" = "ok" ] || return 1
  status="$(printf '%s\n' "$OBSERVATION_FACTS" | awk -F'|' '{print $1}')"
  [ "$status" = "running" ] || return 1
  if [ -n "$expected_image" ]; then
    actual_image="$(printf '%s\n' "$OBSERVATION_FACTS" | awk -F'|' '{print $3}')"
    [ "$actual_image" = "$expected_image" ] || return 1
  fi
}

# 只兼容转换目标服务的普通块映射镜像行；不重写整份YAML，也不猜测别名/流式结构。
# 输入先写入独立候选文件，解析不唯一或镜像来源不符时在重建容器前失败。
prepare_service_compose() {
  awk -v wanted="$SERVICE_NAME" -v key="$3" -v previous="$4" '
    function unquote(value, first, last) {
      first = substr(value, 1, 1)
      last = substr(value, length(value), 1)
      if ((first == "\"" || first == sprintf("%c", 39)) && first == last) {
        return substr(value, 2, length(value) - 2)
      }
      return value
    }
    {
      original = $0
      text = $0
      sub(/\r$/, "", text)
      if (text ~ /^[ \t]*($|#)/) { print original; next }
      if (text ~ /^ *\t/) { invalid = 1; print original; next }
      indent = match(text, /[^ ]/) - 1
      content = substr(text, indent + 1)
      if (indent == 0) {
        in_services = (content ~ /^services:[ ]*(#.*)?$/)
        if (in_services) { sections++; service_indent = 0 }
        target = 0
      } else if (in_services) {
        if (!service_indent) service_indent = indent
        if (indent == service_indent) {
          name = content
          sub(/:.*/, "", name)
          target = (unquote(name) == wanted)
          property_indent = 0
          if (target) {
            targets++
            if (content !~ /^[^:]+:[ ]*(#.*)?$/) invalid = 1
          }
        } else if (target && indent > service_indent) {
          if (!property_indent) property_indent = indent
          if (indent == property_indent && content ~ /^image:[ ]*/) {
            images++
            value = content
            sub(/^image:[ ]*/, "", value)
            comment = ""
            if (match(value, /[ ]+#/)) {
              comment = substr(value, RSTART)
              value = substr(value, 1, RSTART - 1)
            }
            sub(/[ ]+$/, "", value)
            value = unquote(value)
            if (value != previous && value != ("$" key) && value !~ ("^[$][{]" key "(:?[-?][^}]*)?[}]$")) invalid = 1
            original = substr(text, 1, indent) "image: ${" key "}" comment
          }
        }
      }
      print original
    }
    END { if (invalid || sections != 1 || targets != 1 || images != 1) exit 1 }
  ' "$1" > "$2"
}

rollback_service_upgrade() {
  service_dir="$1"
  backup_env="$2"
  previous_image="$3"
  event "rollback" "running" "restoring previous service configuration"
  cp "$backup_env" "$service_dir/.env" || return 1
  cp "${backup_env%/*}/docker-compose.yml" "$service_dir/docker-compose.yml" || return 1
  (cd "$service_dir" && compose -f docker-compose.yml up -d --no-deps --force-recreate "$SERVICE_NAME") || return 1
  verify_service_state "$service_dir" "$previous_image" || return 1
  event "rollback" "success" "previous service image restored"
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
  [ -f "$current_dir/docker-compose.yml" ] || fail "service_upgrade" "docker-compose.yml is missing" 74
  [ -f "$current_dir/.env" ] || fail "service_upgrade" ".env is missing" 75
  cp -a "$current_dir/docker-compose.yml" "$backup_dir/" || fail "service_upgrade" "failed to backup compose" 76
  cp -a "$current_dir/.env" "$backup_dir/.env.before" || fail "service_upgrade" "failed to backup env" 77
  if [ -f "$current_dir/manifest.json" ]; then
    cp -a "$current_dir/manifest.json" "$backup_dir/" || fail "service_upgrade" "failed to backup manifest" 78
  fi
  candidate_compose="$upgrade_dir/docker-compose.after.yml"
  prepare_service_compose "$current_dir/docker-compose.yml" "$candidate_compose" "$image_key" "$current_image" ||
    fail "service_upgrade" "无法安全定位目标服务镜像；当前Compose需使用唯一的服务块和image声明，未重建容器" 82
  cp -a "$REMOTE_IMAGE" "$upgrade_dir/$(basename "$REMOTE_IMAGE")" || fail "service_upgrade" "failed to snapshot image" 79

  event "load_image" "running" "$SERVICE_IMAGE"
  load_image_archive_with_tag "$REMOTE_IMAGE" "$SERVICE_IMAGE" ||
    fail "load_image" "loaded image cannot be assigned expected tag: ${SERVICE_IMAGE}" 73

  if ! replace_env_value "$current_dir/.env" "$image_key" "$SERVICE_IMAGE" ||
    ! cp "$candidate_compose" "$current_dir/docker-compose.yml"; then
    cp "$backup_dir/.env.before" "$current_dir/.env" &&
      cp "$backup_dir/docker-compose.yml" "$current_dir/docker-compose.yml" ||
      fail "rollback" "failed to restore configuration before service start" 83
    fail "service_upgrade" "failed to update service configuration; original configuration restored" 84
  fi
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
  if ! (cd "$current_dir" && compose -f docker-compose.yml up -d --no-deps --force-recreate "$SERVICE_NAME"); then
    if ! rollback_service_upgrade "$current_dir" "$backup_dir/.env.before" "$current_image"; then
      fail "rollback" "service start failed and rollback failed" 83
    fi
    fail "service_upgrade" "service start failed; previous image restored" 84
  fi
  if ! verify_service_state "$current_dir" "$SERVICE_IMAGE"; then
    if ! rollback_service_upgrade "$current_dir" "$backup_dir/.env.before" "$current_image"; then
      fail "rollback" "service health failed and rollback failed" 85
    fi
    fail "service_upgrade" "service health failed; previous image restored" 86
  fi
  event "service_upgrade" "success" "service=${SERVICE_NAME}; image=${SERVICE_IMAGE}; backup=${backup_dir}"
}

# 只读观测使用独立子Shell，避免采集局部变量影响既有部署与回滚函数。
# Compose config 已完成 .env 插值；仅从规范化输出提取当前服务的镜像。
configured_service_image() {
  awk -v wanted="$1" '
    function unquote(value, quote) {
      quote = substr(value, 1, 1)
      if ((quote == "\"" || quote == sprintf("%c", 39)) && substr(value, length(value), 1) == quote)
        return substr(value, 2, length(value) - 2)
      return value
    }
    /^services: *$/ { in_services = 1; next }
    /^[^ ]/ { in_services = 0 }
    in_services && /^  [^ ]/ {
      service = $0; sub(/^  /, "", service); sub(/: *$/, "", service)
      service = unquote(service)
    }
    in_services && service == wanted && /^    image:/ {
      value = $0; sub(/^    image: */, "", value); sub(/\r$/, "", value)
      print unquote(value); exit
    }
  '
}

json_optional() {
  if [ -n "$1" ]; then printf '"%s"' "$(json_escape "$1")"; else printf 'null'; fi
}

# 每轮既有验证重置一次；同轮结果供最终报告复用，避免再次读取容器状态。
reset_runtime_observation() {
  OBSERVATION_RELEASE="$1"
  OBSERVATION_CACHE=""
  OBSERVATION_SERVICES=""
}

observed_service_names() {
  if [ "${OBSERVATION_RELEASE:-}" = "$1" ] && [ -n "${OBSERVATION_SERVICES:-}" ]; then
    printf '%s\n' "$OBSERVATION_SERVICES"
  else
    (cd "$1" && compose -f docker-compose.yml config --services 2>/dev/null)
  fi
}

observe_runtime_service() {
  if [ "${OBSERVATION_RELEASE:-}" != "$1" ]; then reset_runtime_observation "$1"; fi
  OBSERVATION_ROW="$(printf '%s\n' "${OBSERVATION_CACHE:-}" | awk -F'|' -v wanted="$2" '$1 == wanted {sub(/^[^|]*[|]/, ""); print; exit}')"
  if [ -n "$OBSERVATION_ROW" ]; then
    OBSERVATION_RESULT="${OBSERVATION_ROW%%|*}"
    OBSERVATION_FACTS="${OBSERVATION_ROW#*|}"
    return 0
  fi
  OBSERVATION_RESULT="ok"
  OBSERVATION_FACTS=""
  if ! OBSERVATION_CONTAINERS="$(
      cd "$1" || exit 1
      if [ "${3:-all}" = "verification" ]; then
        compose -f docker-compose.yml ps -q "$2" 2>/dev/null
      else
        compose -f docker-compose.yml ps -a -q "$2" 2>/dev/null
      fi
    )"; then
    OBSERVATION_RESULT="query_failed"
  elif [ -z "$OBSERVATION_CONTAINERS" ]; then
    OBSERVATION_RESULT="missing"
  elif [ "$(printf '%s\n' "$OBSERVATION_CONTAINERS" | awk 'NF {count++} END {print count+0}')" != "1" ]; then
    OBSERVATION_RESULT="ambiguous"
  elif ! OBSERVATION_FACTS="$(docker inspect -f '{{.State.Status}}|{{if .State.Health}}{{.State.Health.Status}}{{end}}|{{.Config.Image}}|{{.Image}}' "$OBSERVATION_CONTAINERS" 2>/dev/null)"; then
    OBSERVATION_RESULT="inspect_failed"
  fi
  OBSERVATION_CACHE="${OBSERVATION_CACHE:-}
$2|$OBSERVATION_RESULT|$OBSERVATION_FACTS"
}

inspect_services() (
  started_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  source="$SERVICE_CHECK_SOURCE"
  scope="all"
  [ -z "$SERVICE_NAME" ] || scope="service"
  check_state="succeeded"
  check_error=""
  rows=""
  expected_services=""
  separator=""
  current_dir="$(current_release_dir)"
  if [ -z "$current_dir" ] || [ ! -f "$current_dir/docker-compose.yml" ]; then
    check_error="当前一体机缺少生效Compose，无法检查服务"
  elif ! configured="$(cd "$current_dir" && compose -f docker-compose.yml config 2>/dev/null)"; then
    check_error="无法解析当前生效Compose"
  elif ! services="$(observed_service_names "$current_dir")" || [ -z "$services" ]; then
    check_error="当前生效Compose未返回服务集合"
  else
    for service in $services; do
      [ -z "$expected_services" ] || expected_services="${expected_services},"
      expected_services="${expected_services}\"$(json_escape "$service")\""
    done
    if [ -n "$SERVICE_NAME" ]; then
      found="false"
      for service in $services; do [ "$service" != "$SERVICE_NAME" ] || found="true"; done
      if [ "$found" = "true" ]; then services="$SERVICE_NAME"; else check_error="目标服务不在当前生效Compose中"; fi
    fi
    if [ -z "$check_error" ]; then
      for service in $services; do
        expected_image="$(printf '%s\n' "$configured" | configured_service_image "$service")"
        runtime_state="unknown"; health_status=""; actual_image=""; image_id=""
        state="unknown"; detail=""
        observe_runtime_service "$current_dir" "$service"
        if [ "$OBSERVATION_RESULT" = "query_failed" ]; then
          detail="读取服务容器失败"
          check_error="部分服务的容器查询失败，保留上次有效检查记录"
        elif [ "$OBSERVATION_RESULT" = "missing" ]; then
          runtime_state="missing"; state="abnormal"; detail="Compose未返回可检查的服务容器"
        else
          if [ "$OBSERVATION_RESULT" = "ambiguous" ]; then
            detail="服务存在多个容器，无法确定唯一运行版本"
          elif [ "$OBSERVATION_RESULT" = "ok" ]; then
            facts="$OBSERVATION_FACTS"
            runtime_state="$(printf '%s\n' "$facts" | awk -F'|' '{print $1}')"
            health_status="$(printf '%s\n' "$facts" | awk -F'|' '{print $2}')"
            actual_image="$(printf '%s\n' "$facts" | awk -F'|' '{print $3}')"
            image_id="$(printf '%s\n' "$facts" | awk -F'|' '{print $4}')"
            if [ -z "$runtime_state" ] || [ -z "$actual_image" ] || [ -z "$image_id" ]; then
              state="unknown"; detail="容器检查返回的运行事实不完整"
              check_error="部分服务的运行事实不完整，保留上次有效检查记录"
            elif [ "$runtime_state" != "running" ]; then
              state="abnormal"; detail="容器未运行"
            elif [ -n "$health_status" ] && [ "$health_status" != "healthy" ]; then
              state="abnormal"; detail="容器健康检查未通过"
            elif [ -z "$expected_image" ]; then
              state="unknown"; detail="无法读取生效Compose中的期望镜像"
            elif [ "$actual_image" != "$expected_image" ]; then
              state="version_mismatch"; detail="实际镜像与生效Compose不一致"
            else
              state="normal"
            fi
          else
            detail="读取容器运行状态和镜像失败"
            check_error="部分服务的运行事实读取失败，保留上次有效检查记录"
          fi
        fi
        checked_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        row="$(printf '{"serviceName":"%s","state":"%s","runtimeState":"%s","healthStatus":%s,"expectedImage":%s,"actualImage":%s,"imageId":%s,"message":%s,"checkedAt":"%s","source":"%s"}' \
          "$(json_escape "$service")" "$state" "$(json_escape "$runtime_state")" \
          "$(json_optional "$health_status")" "$(json_optional "$expected_image")" \
          "$(json_optional "$actual_image")" "$(json_optional "$image_id")" "$(json_optional "$detail")" "$checked_at" "$source")"
        rows="${rows}${separator}${row}"; separator=","
      done
    fi
  fi
  [ -z "$check_error" ] || check_state="failed"
  checked_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  printf '{"step":"service_observation","status":"info","message":"服务运行事实采集完成","report":{"startedAt":"%s","checkedAt":"%s","source":"%s","scope":"%s","serviceName":%s,"expectedServices":[%s],"services":[%s],"state":"%s","error":%s}}\n' \
    "$started_at" "$checked_at" "$source" "$scope" "$(json_optional "$SERVICE_NAME")" "$expected_services" "$rows" "$check_state" "$(json_optional "$check_error")"
)

service_health() {
  current_dir="$(require_service_context "service_health")"
  image_key="$(service_image_env_key)"
  expected_image="$SERVICE_IMAGE"
  if [ -z "$expected_image" ]; then
    expected_image="$(env_value "$current_dir/.env" "$image_key" 2>/dev/null || true)"
  fi
  verify_service_state "$current_dir" "$expected_image" || fail "service_health" "service state or image mismatch" 80
  (cd "$current_dir" && compose -f docker-compose.yml ps "$SERVICE_NAME")
  inspect_services
  event "service_health" "success" "service=${SERVICE_NAME}; image=${expected_image}"
}

health() {
  event "health" "running" "checking containers"
  current_dir="$(current_release_dir)"
  [ -n "$current_dir" ] || fail "health" "current release does not exist" 50
  wait_for_release "$current_dir" || fail "health" "release service, version or fingerprint check failed" 50
  (cd "$current_dir" && compose -f docker-compose.yml ps)
  inspect_services
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
  inspect-services)
    inspect_services
    ;;
  *)
    echo "usage: edge-node-agent.sh version|precheck|install|backup|health|service-check|service-upgrade|service-health|inspect-services" >&2
    exit 2
    ;;
esac
