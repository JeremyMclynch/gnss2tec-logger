#!/bin/sh
# gnss2tec-logger external-HDD backup: collect-once with deferred cull.
#
# Copies completed archive days to a swappable external drive (mounted on demand
# by autofs at /mnt/<label>), recording each transferred day in a persistent
# internal log. The log is the source of truth for "already collected": days in
# it are never re-pushed to a freshly-swapped empty drive, and only logged days
# may be removed from internal storage. Configuration comes from runtime.env
# (loaded by the systemd unit via EnvironmentFile); every value has a default.
#
# Exits 0 on success AND on the common "drive not present" case (a skip), so a
# missing drive between swaps is never treated as a failure.
set -eu

log() { echo "gnss2tec-backup: $*"; }

ARCHIVE_DIR="${GNSS2TEC_ARCHIVE_DIR:-/var/lib/gnss2tec-logger/archive}"
LABEL="${GNSS2TEC_BACKUP_LABEL:-rinexbackup}"
MODE="${GNSS2TEC_BACKUP_DELETE_MODE:-rotate}"
MIN_FREE_MB="${GNSS2TEC_BACKUP_MIN_FREE_MB:-2048}"
DEST="/mnt/${LABEL}"
STATE_DIR="/var/lib/gnss2tec-logger/backup"
LOG_FILE="${STATE_DIR}/transferred.log"

# Robust, filesystem-agnostic rsync: do not try to preserve Unix perms/ownership
# (exFAT/NTFS/FAT cannot store them), sync by time+size with a FAT-safe window,
# and resume partial transfers interrupted by an early drive pull.
RSYNC_OPTS="-rt --modify-window=2 --no-perms --no-owner --no-group --partial"

if [ -z "$LABEL" ]; then
    log "GNSS2TEC_BACKUP_LABEL is empty; nothing to do."
    exit 0
fi

mkdir -p "$STATE_DIR"
[ -f "$LOG_FILE" ] || : > "$LOG_FILE"

# ---- Drive-present guard ---------------------------------------------------
if [ ! -e "/dev/disk/by-label/${LABEL}" ]; then
    log "backup drive '${LABEL}' not present; skipping (normal between swaps)."
    exit 0
fi

# Touch the path so autofs mounts it on demand, then confirm it really mounted.
ls "$DEST" >/dev/null 2>&1 || true
if ! mountpoint -q "$DEST"; then
    log "drive '${LABEL}' present but '${DEST}' is not mounted; skipping."
    exit 0
fi

if [ ! -d "$ARCHIVE_DIR" ]; then
    log "archive dir '${ARCHIVE_DIR}' does not exist; nothing to back up."
    exit 0
fi

logged() { grep -qxF "$1" "$LOG_FILE" 2>/dev/null; }

remove_internal_day() {
    # Remove an internal archive day dir (relative YYYY/DDD); prune an empty year.
    rm -rf "${ARCHIVE_DIR:?}/${1}"
    rmdir "$(dirname "${ARCHIVE_DIR}/${1}")" 2>/dev/null || true
}

free_mb() {
    df -PBM "$ARCHIVE_DIR" | awk 'NR==2 { v=$4; sub(/M$/,"",v); print v }'
}

TODAY="$(date -u +%Y/%j)"   # current UTC day is still growing; never collect it

# ---- Transfer new completed days ------------------------------------------
transferred=0
# Archive day dirs as relative "YYYY/DDD", oldest first.
DAYS="$(find "$ARCHIVE_DIR" -mindepth 2 -maxdepth 2 -type d 2>/dev/null \
        | sed "s#^${ARCHIVE_DIR%/}/##" | LC_ALL=C sort)"

for rel in $DAYS; do
    [ "$rel" = "$TODAY" ] && continue          # skip the in-progress day
    if logged "$rel"; then                      # already collected on some drive
        if [ "$MODE" = "immediate" ]; then remove_internal_day "$rel"; fi
        continue
    fi

    src="${ARCHIVE_DIR}/${rel}"
    dst="${DEST}/archive/${rel}"
    mkdir -p "$dst"
    if rsync $RSYNC_OPTS "${src}/" "${dst}/"; then
        echo "$rel" >> "$LOG_FILE"
        transferred=$((transferred + 1))
        log "transferred ${rel}"
        if [ "$MODE" = "immediate" ]; then remove_internal_day "$rel"; fi
    else
        log "rsync FAILED for ${rel}; leaving it in place to retry next run."
    fi
done

log "transfer pass complete (${transferred} new day(s))."

# ---- Reclaim internal space (rotate mode) ---------------------------------
if [ "$MODE" = "rotate" ]; then
    SORTED="$(mktemp)"
    LC_ALL=C sort "$LOG_FILE" > "$SORTED"
    while [ "$(free_mb)" -lt "$MIN_FREE_MB" ]; do
        # Oldest logged day still present on internal storage.
        victim=""
        while IFS= read -r rel; do
            if [ -d "${ARCHIVE_DIR}/${rel}" ]; then victim="$rel"; break; fi
        done < "$SORTED"
        if [ -z "$victim" ]; then
            log "internal free below ${MIN_FREE_MB} MB but no collected days left to cull."
            break
        fi
        remove_internal_day "$victim"
        log "culled collected day ${victim} (free now $(free_mb) MB, floor ${MIN_FREE_MB} MB)."
    done
    rm -f "$SORTED"
fi

exit 0
