# Tervin shell integration — fish
#
# fish provides real events, so this is the simplest of the four: no traps, no
# prompt-string surgery beyond wrapping fish_prompt.
#
# Disable with: set -gx TERVIN_SHELL_INTEGRATION 0

status is-interactive; or exit 0
test "$TERM_PROGRAM" = "Tervin"; or exit 0
test "$TERVIN_SHELL_INTEGRATION" = "0"; and exit 0
set -q __tervin_loaded; and exit 0
set -g __tervin_loaded 1

function __tervin_osc
    printf '\033]%s\007' $argv[1]
end

function __tervin_b64
    printf '%s' $argv[1] | base64 | tr -d '\n'
end

function __tervin_report_cwd
    __tervin_osc "7;file://"(hostname)"$PWD"
end

function __tervin_report_git
    test "$TERVIN_REPORT_GIT" = "1"; or return 0
    set -l branch (command git rev-parse --abbrev-ref HEAD 2>/dev/null)
    test -n "$branch"; and __tervin_osc "7373;branch="(__tervin_b64 "$branch")
end

function __tervin_preexec --on-event fish_preexec
    set -g __tervin_executing 1
    __tervin_osc "7373;cmd="(__tervin_b64 "$argv[1]")";shell=fish"
    __tervin_osc "133;C"
end

function __tervin_postexec --on-event fish_postexec
    __tervin_osc "133;D;$status"
    set -e __tervin_executing
end

# fish has no precmd event, so wrap fish_prompt: emit the prompt-start mark
# before the user's prompt renders and the prompt-end mark after it.
if not functions -q __tervin_original_fish_prompt
    functions -c fish_prompt __tervin_original_fish_prompt

    function fish_prompt
        __tervin_report_cwd
        __tervin_report_git
        __tervin_osc "133;A"
        __tervin_original_fish_prompt
        __tervin_osc "133;B"
    end
end

__tervin_report_cwd
__tervin_osc "133;A"
