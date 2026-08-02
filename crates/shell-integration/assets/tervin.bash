# Tervin shell integration — bash
#
# bash has no native preexec hook, so the command boundary is detected with the
# DEBUG trap. The guards below matter: without them the trap fires for every
# command inside PROMPT_COMMAND and for completion, which would produce phantom
# Blocks.
#
# Works on bash 3.2 (still the system bash on macOS) as well as bash 5.
# Disable with: export TERVIN_SHELL_INTEGRATION=0

[[ "$TERM_PROGRAM" == "Tervin" ]] || return 0
[[ "$TERVIN_SHELL_INTEGRATION" == "0" ]] && return 0
[[ -n "$__tervin_loaded" ]] && return 0
__tervin_loaded=1
__tervin_executing=""
# Set while our own prompt hook runs, so the DEBUG trap can ignore it.
__tervin_in_prompt=""

__tervin_osc() { printf '\033]%s\007' "$1"; }
__tervin_b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

__tervin_report_cwd() {
  __tervin_osc "7;file://${HOSTNAME:-localhost}${PWD}"
}

__tervin_report_git() {
  [[ "$TERVIN_REPORT_GIT" == "1" ]] || return 0
  local branch
  branch=$(command git rev-parse --abbrev-ref HEAD 2>/dev/null) || return 0
  [[ -n "$branch" ]] && __tervin_osc "7373;branch=$(__tervin_b64 "$branch")"
}

__tervin_preexec() {
  # Ignore everything that is not a real, interactive submission.
  [[ -n "$COMP_LINE" ]] && return 0        # tab completion
  [[ -n "$__tervin_in_prompt" ]] && return 0  # our own prompt hook
  [[ -n "$__tervin_executing" ]] && return 0  # already inside this command
  case "$BASH_COMMAND" in
    __tervin_*) return 0 ;;                # our own functions
  esac

  __tervin_executing=1
  # Prefer the history entry: BASH_COMMAND is only the current simple command,
  # so a pipeline would otherwise be reported as just its first stage.
  local line
  line=$(HISTTIMEFORMAT= builtin history 1 2>/dev/null | sed 's/^ *[0-9]* *//')
  [[ -z "$line" ]] && line="$BASH_COMMAND"
  __tervin_osc "7373;cmd=$(__tervin_b64 "$line");shell=bash"
  __tervin_osc "133;C"
}

__tervin_precmd() {
  local exit_code=$?
  __tervin_in_prompt=1
  if [[ -n "$__tervin_executing" ]]; then
    __tervin_osc "133;D;${exit_code}"
    __tervin_executing=""
  fi
  __tervin_report_cwd
  __tervin_report_git
  __tervin_osc "133;A"
  __tervin_in_prompt=""
  return $exit_code
}

trap '__tervin_preexec' DEBUG

# Chain onto any existing PROMPT_COMMAND rather than replacing it.
case ";${PROMPT_COMMAND};" in
  *";__tervin_precmd;"*) ;;
  *)
    if [[ -z "$PROMPT_COMMAND" ]]; then
      PROMPT_COMMAND="__tervin_precmd"
    else
      PROMPT_COMMAND="__tervin_precmd;${PROMPT_COMMAND}"
    fi
    ;;
esac

# \[ \] marks these bytes as zero-width so readline's cursor maths stays right.
if [[ "$PS1" != *"133;B"* ]]; then
  PS1="${PS1}\[\033]133;B\007\]"
fi

__tervin_report_cwd
__tervin_osc "133;A"
