#!/usr/bin/env bash

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INPUT_FILE="${SCRIPT_DIR}/kernel-test.md"
RESULT_FILE="${SCRIPT_DIR}/kernel-test-result.md"
LOG_DIR="${SCRIPT_DIR}/.kernel-test-logs"

if [[ ! -f "${INPUT_FILE}" ]]; then
  echo "Input file not found: ${INPUT_FILE}" >&2
  exit 1
fi

mkdir -p "${LOG_DIR}"
BODY_FILE="$(mktemp)"
trap 'rm -f "${BODY_FILE}"' EXIT

trim() {
  local value="$1"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "${value}"
}

strip_ansi() {
  sed -E 's/\x1B\[[0-9;]*[A-Za-z]//g'
}

case_index=0
executed=0
passed=0
failed=0
skipped=0

result_exists=false
if [[ -f "${RESULT_FILE}" ]]; then
  result_exists=true
fi

case_log_file() {
  local case_name="$1"
  printf '%s/%s.log' "${LOG_DIR}" "${case_name}"
}

has_case_in_log() {
  local case_name="$1"
  local log_file="$2"
  local any_log
  local expected_line

  expected_line="test ${case_name} ..."

  if [[ -f "${log_file}" ]] && grep -Fq "${expected_line}" "${log_file}"; then
    return 0
  fi

  for any_log in "${LOG_DIR}"/*.log; do
    [[ -f "${any_log}" ]] || continue
    if grep -Fq "${expected_line}" "${any_log}"; then
      return 0
    fi
  done

  return 1
}

while IFS= read -r line || [[ -n "${line}" ]]; do
  case_name="$(trim "${line}")"

  if [[ -z "${case_name}" ]]; then
    continue
  fi

  ((case_index += 1))

  log_file="$(case_log_file "${case_name}")"

  if has_case_in_log "${case_name}" "${log_file}"; then
    ((skipped += 1))
    echo "[${case_index}] Skipping ${case_name} (already in case log)"
    continue
  fi

  ((executed += 1))

  echo "[${case_index}] Running ${case_name}"

  if make miri_test_kernel CASE="${case_name}" >"${log_file}" 2>&1; then
    ((passed += 1))
    {
      echo "## ${case_name}"
      echo "- Status: PASS"
      echo
    } >>"${BODY_FILE}"
  else
    exit_code=$?
    ((failed += 1))
    {
      echo "## ${case_name}"
      echo "- Status: FAIL (exit code: ${exit_code})"
      echo
      echo '```text'
      strip_ansi <"${log_file}"
      echo '```'
      echo
    } >>"${BODY_FILE}"
  fi
done <"${INPUT_FILE}"

if [[ "${result_exists}" == true ]]; then
  {
    echo
    echo "---"
    echo
    echo "# Kernel Miri Test Result (Incremental)"
    echo
    echo "- Generated at (UTC): $(date -u '+%Y-%m-%d %H:%M:%S')"
    echo "- Total Cases: ${case_index}"
    echo "- Executed: ${executed}"
    echo "- Passed: ${passed}"
    echo "- Failed: ${failed}"
    echo "- Skipped (already recorded): ${skipped}"
    echo
    cat "${BODY_FILE}"
  } >>"${RESULT_FILE}"
else
  {
    echo "# Kernel Miri Test Result"
    echo
    echo "- Generated at (UTC): $(date -u '+%Y-%m-%d %H:%M:%S')"
    echo "- Total Cases: ${case_index}"
    echo "- Executed: ${executed}"
    echo "- Passed: ${passed}"
    echo "- Failed: ${failed}"
    echo "- Skipped (already recorded): ${skipped}"
    echo
    cat "${BODY_FILE}"
  } >"${RESULT_FILE}"
fi

echo "Result saved to ${RESULT_FILE}"

if [[ ${failed} -gt 0 ]]; then
  exit 1
fi

