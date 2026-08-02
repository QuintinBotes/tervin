# Tervin shell integration — zsh
#
# Reports prompt boundaries, the submitted command, exit status, and the working
# directory using OSC 133 (semantic prompt), OSC 7 (cwd), and OSC 7373 (Tervin).
#
# Safe to source anywhere: it does nothing unless running inside Tervin, and it
# never replaces an existing hook — it appends to zsh's hook arrays.
#
# Disable at any time with: export TERVIN_SHELL_INTEGRATION=0

# Only participate inside Tervin, and only once per shell.
[[ "$TERM_PROGRAM" == "Tervin" ]] || return 0
[[ "$TERVIN_SHELL_INTEGRATION" == "0" ]] && return 0
[[ -n "$__tervin_loaded" ]] && return 0
typeset -g __tervin_loaded=1
typeset -g __tervin_executing=""

__tervin_osc() { printf '\033]%s\007' "$1"; }

# Values that may contain ';', newlines, or quotes travel base64-encoded.
__tervin_b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

__tervin_report_cwd() {
  __tervin_osc "7;file://${HOST:-localhost}${PWD}"
}

# Optional: branch reporting costs a `git` invocation per prompt, so it is
# off by default. Tervin resolves Git state itself, off the prompt path.
#   export TERVIN_REPORT_GIT=1
__tervin_report_git() {
  [[ "$TERVIN_REPORT_GIT" == "1" ]] || return 0
  local branch
  branch=$(command git rev-parse --abbrev-ref HEAD 2>/dev/null) || return 0
  [[ -n "$branch" ]] && __tervin_osc "7373;branch=$(__tervin_b64 "$branch")"
}

# Runs after a command is accepted, before it executes. $1 is the command line.
__tervin_preexec() {
  __tervin_executing=1
  __tervin_osc "7373;cmd=$(__tervin_b64 "$1");shell=zsh"
  __tervin_osc "133;C"
}

# Runs before each prompt is drawn.
__tervin_precmd() {
  local exit_code=$?
  # Only close a command if one actually ran; a bare Enter must not create a Block.
  if [[ -n "$__tervin_executing" ]]; then
    __tervin_osc "133;D;${exit_code}"
    __tervin_executing=""
  fi
  __tervin_report_cwd
  __tervin_report_git
  __tervin_osc "133;A"
}

autoload -Uz add-zsh-hook 2>/dev/null
if (( $+functions[add-zsh-hook] )); then
  add-zsh-hook preexec __tervin_preexec
  add-zsh-hook precmd  __tervin_precmd
else
  # Very old zsh without add-zsh-hook: fall back to direct arrays.
  typeset -ga preexec_functions precmd_functions
  preexec_functions+=(__tervin_preexec)
  precmd_functions+=(__tervin_precmd)
fi

# Mark where the prompt ends and typing begins. %{ %} tells zsh these bytes
# occupy no columns, which keeps line editing and reflow correct.
if [[ "$PS1" != *"133;B"* ]]; then
  PS1="${PS1}"$'%{\033]133;B\007%}'
fi

# Emit an initial cwd and prompt mark so the first Block is complete even if the
# shell was already at a prompt when integration loaded.
__tervin_report_cwd
__tervin_osc "133;A"
