#!/usr/bin/env bash
# Orchestrate the multi-sequencer DbElected smoke (chain#412).
# See README.md for what this validates.
#
# Usage:
#   ./run.sh up           start postgres + 3 ligate-node containers
#   ./run.sh status       show current role of each instance + chain head
#   ./run.sh kill-leader  SIGKILL the BatchProducer container
#   ./run.sh smoke        full sequence: up, verify election, kill leader,
#                         verify failover, then down
#   ./run.sh down         stop containers + drop volumes

set -euo pipefail

cd "$(dirname "$0")"

COMPOSE="docker compose"
PORTS=(12346 12347 12348)
NODES=(ligate-1 ligate-2 ligate-3)

cmd="${1:-smoke}"

#
# Helpers
#

wait_for_healthy() {
  echo "==> waiting for all containers to report healthy..."
  local deadline=$(($(date +%s) + 120))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    local all_up=1
    for node in "${NODES[@]}"; do
      local status
      status="$($COMPOSE ps --format json "$node" 2>/dev/null | jq -r '.Health // ""')"
      if [ "$status" != "healthy" ]; then
        all_up=0
        break
      fi
    done
    if [ "$all_up" -eq 1 ]; then
      echo "==> all containers healthy"
      return 0
    fi
    sleep 2
  done
  echo "ERROR: containers did not reach healthy within 120s" >&2
  $COMPOSE ps
  return 1
}

get_role() {
  local port="$1"
  curl -sS --max-time 3 "http://localhost:$port/v1/sequencer/role" 2>/dev/null \
    | jq -r '. // "unknown"' \
    || echo "unreachable"
}

get_head_height() {
  local port="$1"
  curl -sS --max-time 3 "http://localhost:$port/v1/rollup/info" 2>/dev/null \
    | jq -r '.latest_block // 0' \
    || echo "0"
}

show_roles() {
  echo ""
  echo "===================== current cluster state ====================="
  printf "%-10s %-10s %-20s %-20s\n" "NODE" "PORT" "ROLE" "HEAD_HEIGHT"
  for i in "${!NODES[@]}"; do
    local node="${NODES[$i]}"
    local port="${PORTS[$i]}"
    printf "%-10s %-10s %-20s %-20s\n" "$node" "$port" "$(get_role "$port")" "$(get_head_height "$port")"
  done
  echo "================================================================="
  echo ""
}

find_leader() {
  for i in "${!NODES[@]}"; do
    local node="${NODES[$i]}"
    local port="${PORTS[$i]}"
    if [ "$(get_role "$port")" = "BatchProducer" ]; then
      echo "$node"
      return 0
    fi
  done
  echo ""
  return 1
}

verify_one_leader() {
  echo "==> verifying exactly one leader..."
  local leader_count=0
  local replica_count=0
  for port in "${PORTS[@]}"; do
    local role
    role="$(get_role "$port")"
    case "$role" in
      BatchProducer)
        leader_count=$((leader_count + 1))
        ;;
      PgSyncReplica|DaOnlyReplica)
        replica_count=$((replica_count + 1))
        ;;
      *)
        echo "ERROR: instance on :$port returned unexpected role '$role'" >&2
        return 1
        ;;
    esac
  done
  if [ "$leader_count" -ne 1 ]; then
    echo "ERROR: expected exactly 1 BatchProducer, found $leader_count" >&2
    return 1
  fi
  if [ "$replica_count" -ne 2 ]; then
    echo "ERROR: expected exactly 2 replicas, found $replica_count" >&2
    return 1
  fi
  echo "==> exactly 1 leader + 2 replicas, as expected"
}

#
# Sub-commands
#

cmd_up() {
  $COMPOSE up -d
  wait_for_healthy
  # give the heartbeat task a moment to elect a leader
  sleep 3
  show_roles
}

cmd_status() {
  show_roles
}

cmd_kill_leader() {
  local leader
  leader="$(find_leader)" || { echo "ERROR: no current leader found" >&2; return 1; }
  echo "==> killing leader: $leader"
  $COMPOSE kill -s SIGKILL "$leader"
  echo "==> killed; waiting for re-election (default SDK leader_timeout 500ms + grace 10s)..."
  sleep 12
}

cmd_smoke() {
  echo "==> step 1: bring up the stack"
  cmd_up

  echo "==> step 2: verify initial election (1 leader + 2 replicas)"
  verify_one_leader
  local initial_leader
  initial_leader="$(find_leader)"
  echo "==> initial leader: $initial_leader"

  echo "==> step 3: kill leader, wait for re-election"
  cmd_kill_leader

  echo "==> step 4: verify exactly one new leader (and not the killed one)"
  show_roles
  # The killed container is gone; only 2 instances are reachable now.
  # We expect exactly 1 BatchProducer among the 2 survivors.
  local new_leader_count=0
  for i in "${!NODES[@]}"; do
    local node="${NODES[$i]}"
    local port="${PORTS[$i]}"
    if [ "$node" = "$initial_leader" ]; then
      continue  # killed
    fi
    if [ "$(get_role "$port")" = "BatchProducer" ]; then
      new_leader_count=$((new_leader_count + 1))
    fi
  done
  if [ "$new_leader_count" -ne 1 ]; then
    echo "ERROR: expected exactly 1 new leader among 2 survivors, found $new_leader_count" >&2
    return 1
  fi
  echo "==> SMOKE PASSED"
  echo ""
  echo "==> step 5: teardown"
  cmd_down
}

cmd_down() {
  $COMPOSE down -v
}

#
# Dispatcher
#

case "$cmd" in
  up)          cmd_up ;;
  status)      cmd_status ;;
  kill-leader) cmd_kill_leader ;;
  smoke)       cmd_smoke ;;
  down)        cmd_down ;;
  *)
    echo "unknown subcommand: $cmd" >&2
    echo "usage: $0 {up|status|kill-leader|smoke|down}" >&2
    exit 1
    ;;
esac
