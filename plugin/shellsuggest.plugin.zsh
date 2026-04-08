# shellsuggest - cwd-aware inline suggestion engine for zsh

# Guard: don't load twice
(( ${+_SHELLSUGGEST_LOADED} )) && return
typeset -g _SHELLSUGGEST_LOADED=1

# Check for binary
if ! command -v shellsuggest &>/dev/null; then
    echo "shellsuggest: binary not found in PATH. Install with: cargo install shellsuggest" >&2
    return 1
fi

# Warn if zsh-autosuggestions is also loaded
if (( ${+_ZSH_AUTOSUGGEST_INITIALIZED} )); then
    echo "shellsuggest: warning: zsh-autosuggestions is also loaded. Ghost text will conflict." >&2
fi

# State
typeset -gi _SHELLSUGGEST_ENABLED=1
typeset -g _SHELLSUGGEST_SUGGESTION=""
typeset -g _SHELLSUGGEST_REQUEST_ID=0
typeset -g _SHELLSUGGEST_SESSION_ID="$$-${RANDOM}"
typeset -g _SHELLSUGGEST_LAST_BUFFER=""
typeset -g _SHELLSUGGEST_COPROC_PID=""
typeset -g _SHELLSUGGEST_REGION_HIGHLIGHT=""
typeset -g _SHELLSUGGEST_HIGHLIGHT_STYLE="${SHELLSUGGEST_HIGHLIGHT_STYLE:-${ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE:-fg=8}}"
typeset -g _SHELLSUGGEST_HIGHLIGHT_MEMO="memo=shellsuggest"
typeset -g _SHELLSUGGEST_PENDING_COMMAND=""
typeset -g _SHELLSUGGEST_PENDING_CWD=""
typeset -g _SHELLSUGGEST_LAST_COMMAND=""
typeset -g _SHELLSUGGEST_SUGGESTION_SOURCE=""
typeset -g _SHELLSUGGEST_SUGGESTION_SCORE="0"
typeset -g _SHELLSUGGEST_CANDIDATE_COUNT=0
typeset -g _SHELLSUGGEST_CANDIDATE_INDEX=0
typeset -g _SHELLSUGGEST_HISTORY_IGNORE="${SHELLSUGGEST_HISTORY_IGNORE:-${ZSH_AUTOSUGGEST_HISTORY_IGNORE:-}}"
typeset -g _SHELLSUGGEST_BUFFER_MAX_SIZE="${SHELLSUGGEST_BUFFER_MAX_SIZE:-${ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE:-}}"
typeset -g _SHELLSUGGEST_MANUAL_REBIND="${SHELLSUGGEST_MANUAL_REBIND:-${ZSH_AUTOSUGGEST_MANUAL_REBIND:-}}"

_shellsuggest_invoke_original_widget() {
    (( $# )) || return 1

    local widget_name="$1"
    shift

    if (( ${+widgets[$widget_name]} )); then
        zle "$widget_name" -- "$@"
        return $?
    fi

    return 1
}

_shellsuggest_clear_highlight() {
    local entry
    local -a remaining_highlights=()

    for entry in "${region_highlight[@]}"; do
        [[ "$entry" == *"${_SHELLSUGGEST_HIGHLIGHT_MEMO}"* ]] && continue
        remaining_highlights+=("$entry")
    done

    region_highlight=("${remaining_highlights[@]}")
    _SHELLSUGGEST_REGION_HIGHLIGHT=""
}

_shellsuggest_apply_highlight() {
    _shellsuggest_clear_highlight

    if [[ -z "$POSTDISPLAY" ]]; then
        return
    fi

    local start=${#BUFFER}
    local end=$(( start + ${#POSTDISPLAY} ))
    _SHELLSUGGEST_REGION_HIGHLIGHT="${start} ${end} ${_SHELLSUGGEST_HIGHLIGHT_STYLE} ${_SHELLSUGGEST_HIGHLIGHT_MEMO}"
    region_highlight+=("$_SHELLSUGGEST_REGION_HIGHLIGHT")
}

_shellsuggest_clear_suggestion() {
    _SHELLSUGGEST_SUGGESTION=""
    _SHELLSUGGEST_SUGGESTION_SOURCE=""
    _SHELLSUGGEST_SUGGESTION_SCORE="0"
    _SHELLSUGGEST_CANDIDATE_COUNT=0
    _SHELLSUGGEST_CANDIDATE_INDEX=0
    POSTDISPLAY=""
    _shellsuggest_clear_highlight
}

_shellsuggest_set_suggestion() {
    local suggestion="$1"
    local source="$2"
    local score="$3"
    local candidate_count="$4"
    local candidate_index="$5"
    _SHELLSUGGEST_SUGGESTION="$suggestion"
    _SHELLSUGGEST_SUGGESTION_SOURCE="$source"
    _SHELLSUGGEST_SUGGESTION_SCORE="$score"
    _SHELLSUGGEST_CANDIDATE_COUNT="$candidate_count"
    _SHELLSUGGEST_CANDIDATE_INDEX="$candidate_index"
    POSTDISPLAY="${suggestion#$BUFFER}"
    _shellsuggest_apply_highlight
}

_shellsuggest_refresh_suggestion_for_buffer() {
    if [[ -z "$_SHELLSUGGEST_SUGGESTION" ]]; then
        return
    fi

    if [[ "$_SHELLSUGGEST_SUGGESTION" == "$BUFFER"* && "$_SHELLSUGGEST_SUGGESTION" != "$BUFFER" ]]; then
        POSTDISPLAY="${_SHELLSUGGEST_SUGGESTION#$BUFFER}"
        _shellsuggest_apply_highlight
    else
        _shellsuggest_clear_suggestion
    fi
}

_shellsuggest_buffer_limit_exceeded() {
    local limit="$_SHELLSUGGEST_BUFFER_MAX_SIZE"

    [[ -z "$limit" ]] && return 1
    [[ "$limit" != <-> ]] && return 1

    (( limit > 0 && ${#BUFFER} > limit ))
}

_shellsuggest_should_ignore_suggestion() {
    local suggestion="$1"
    local pattern="$_SHELLSUGGEST_HISTORY_IGNORE"

    [[ -z "$pattern" ]] && return 1

    emulate -L zsh
    setopt localoptions extendedglob
    [[ "$suggestion" == ${~pattern} ]]
}

_shellsuggest_line_escape() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//$'\t'/\\t}"
    value="${value//$'\n'/\\n}"
    value="${value//$'\r'/\\r}"
    print -r -- "$value"
}

_shellsuggest_line_unescape() {
    local value="$1"
    local sentinel=$'\x1f'

    value="${value//\\\\/${sentinel}}"
    value="${value//\\t/$'\t'}"
    value="${value//\\n/$'\n'}"
    value="${value//\\r/$'\r'}"
    value="${value//${sentinel}/\\}"
    print -r -- "$value"
}

_shellsuggest_join_fields() {
    local msg="$1"
    shift

    local field
    for field in "$@"; do
        msg+=$'\t'"$field"
    done

    print -r -- "$msg"
}

# Start coproc
_shellsuggest_start_coproc() {
    emulate -L zsh
    setopt localoptions nomonitor

    if [[ -n "$_SHELLSUGGEST_COPROC_PID" ]] && kill -0 "$_SHELLSUGGEST_COPROC_PID" 2>/dev/null; then
        return 0
    fi

    # Start client as coproc (anonymous)
    coproc { shellsuggest query 2>/dev/null }
    _SHELLSUGGEST_COPROC_PID=$!

    # Grab the coproc file descriptors
    exec {_SHELLSUGGEST_FD_IN}<&p
    exec {_SHELLSUGGEST_FD_OUT}>&p
}

_shellsuggest_send_message() {
    local message="$1"

    print -u $_SHELLSUGGEST_FD_OUT "$message" 2>/dev/null || {
        _shellsuggest_start_coproc
        print -u $_SHELLSUGGEST_FD_OUT "$message" 2>/dev/null || return 1
    }
}

_shellsuggest_apply_suggestion_line() {
    local line="$1"
    local -a fields
    local request_id
    local text
    local source
    local score
    local candidate_count
    local candidate_index

    fields=("${(@ps:	:)line}")
    (( ${#fields} >= 7 )) || return 1
    [[ "${fields[1]}" == "s" ]] || return 1

    request_id="${fields[2]}"
    [[ -z "$request_id" || "$request_id" != "$_SHELLSUGGEST_REQUEST_ID" ]] && return 1

    candidate_count="${fields[3]}"
    candidate_index="${fields[4]}"
    score="${fields[5]}"
    source=$(_shellsuggest_line_unescape "${fields[6]}")
    text=$(_shellsuggest_line_unescape "${fields[7]}")

    if [[ -n "$text" && "$text" != "$BUFFER" && "$text" == "$BUFFER"* ]]; then
        if _shellsuggest_should_ignore_suggestion "$text"; then
            _shellsuggest_clear_suggestion
            return 0
        fi

        _shellsuggest_set_suggestion \
            "$text" \
            "$source" \
            "${score:-0}" \
            "${candidate_count:-0}" \
            "${candidate_index:-0}"
    else
        _shellsuggest_clear_suggestion
    fi
}

_shellsuggest_read_suggestion() {
    local target_request_id="$1"
    local line
    local matched_line=""
    local response_request_id
    local -a fields
    local -i attempts=5

    while (( attempts-- > 0 )); do
        while read -r -t 0 -u $_SHELLSUGGEST_FD_IN line 2>/dev/null; do
            fields=("${(@ps:	:)line}")
            [[ ${#fields} -ge 2 ]] || continue
            [[ "${fields[1]}" == "s" ]] || continue
            response_request_id="${fields[2]}"
            [[ -z "$response_request_id" ]] && continue
            [[ "$response_request_id" != "$target_request_id" ]] && continue
            matched_line="$line"
        done

        [[ -n "$matched_line" ]] && {
            print -r -- "$matched_line"
            return 0
        }

        if ! read -r -t 0.01 -u $_SHELLSUGGEST_FD_IN line 2>/dev/null; then
            continue
        fi

        while true; do
            fields=("${(@ps:	:)line}")
            if [[ ${#fields} -ge 2 && "${fields[1]}" == "s" ]]; then
                response_request_id="${fields[2]}"
                if [[ -n "$response_request_id" && "$response_request_id" == "$target_request_id" ]]; then
                    matched_line="$line"
                fi
            fi

            if ! read -r -t 0 -u $_SHELLSUGGEST_FD_IN line 2>/dev/null; then
                break
            fi
        done

        [[ -n "$matched_line" ]] && {
            print -r -- "$matched_line"
            return 0
        }
    done

    [[ -z "$matched_line" ]] && return 1
    print -r -- "$matched_line"
}

_shellsuggest_wait_for_ack() {
    local timeout="${1:-0.02}"
    local line
    local -a fields

    if read -r -t "$timeout" -u $_SHELLSUGGEST_FD_IN line 2>/dev/null; then
        while true; do
            fields=("${(@ps:	:)line}")
            if [[ ${#fields} -ge 1 && ( "${fields[1]}" == "a" || "${fields[1]}" == "e" ) ]]; then
                return 0
            fi

            if ! read -r -t 0 -u $_SHELLSUGGEST_FD_IN line 2>/dev/null; then
                break
            fi
        done
    fi

    return 1
}

_shellsuggest_record_feedback() {
    local accepted="$1"
    local escaped_command
    local escaped_source
    local escaped_session_id
    local message

    [[ -z "$_SHELLSUGGEST_SUGGESTION" || -z "$_SHELLSUGGEST_SUGGESTION_SOURCE" ]] && return

    if [[ "$accepted" == "true" ]]; then
        accepted=1
    else
        accepted=0
    fi

    escaped_command=$(_shellsuggest_line_escape "$_SHELLSUGGEST_SUGGESTION")
    escaped_source=$(_shellsuggest_line_escape "$_SHELLSUGGEST_SUGGESTION_SOURCE")
    escaped_session_id=$(_shellsuggest_line_escape "$_SHELLSUGGEST_SESSION_ID")
    message=$(_shellsuggest_join_fields "f" "$accepted" "${_SHELLSUGGEST_SUGGESTION_SCORE:-0}" "$escaped_session_id" "$escaped_source" "$escaped_command")
    _shellsuggest_send_message "$message" >/dev/null 2>&1
    _shellsuggest_wait_for_ack 0.05 >/dev/null 2>&1
}

_shellsuggest_query() {
    local buffer="$1"
    local cursor="$2"
    local escaped_buffer
    local escaped_cwd
    local escaped_last_command
    local escaped_session_id
    local message
    local line

    (( _SHELLSUGGEST_REQUEST_ID++ ))

    escaped_buffer=$(_shellsuggest_line_escape "$buffer")
    escaped_cwd=$(_shellsuggest_line_escape "$PWD")
    escaped_session_id=$(_shellsuggest_line_escape "$_SHELLSUGGEST_SESSION_ID")
    if [[ -n "$_SHELLSUGGEST_LAST_COMMAND" ]]; then
        escaped_last_command=$(_shellsuggest_line_escape "$_SHELLSUGGEST_LAST_COMMAND")
    else
        escaped_last_command=""
    fi
    message=$(_shellsuggest_join_fields "s" "$_SHELLSUGGEST_REQUEST_ID" "$cursor" "$escaped_session_id" "$escaped_cwd" "$escaped_buffer" "$escaped_last_command")

    _shellsuggest_send_message "$message" || return 1
    line=$(_shellsuggest_read_suggestion "$_SHELLSUGGEST_REQUEST_ID") || return 1
    _shellsuggest_apply_suggestion_line "$line"
}

_shellsuggest_cycle() {
    local direction="$1"
    local escaped_session_id
    local message
    local line

    [[ -z "$_SHELLSUGGEST_SUGGESTION" ]] && return 0

    escaped_session_id=$(_shellsuggest_line_escape "$_SHELLSUGGEST_SESSION_ID")
    message=$(_shellsuggest_join_fields "c" "$escaped_session_id" "$([[ "$direction" == "next" ]] && print -r -- "n" || print -r -- "p")")
    _shellsuggest_send_message "$message" || return 1
    line=$(_shellsuggest_read_suggestion "$_SHELLSUGGEST_REQUEST_ID") || return 1
    _shellsuggest_apply_suggestion_line "$line"
    zle -R
}

# Main suggest hook — called on buffer changes via zle-line-pre-redraw
_shellsuggest_suggest() {
    [[ "$BUFFER" == "$_SHELLSUGGEST_LAST_BUFFER" ]] && return
    _SHELLSUGGEST_LAST_BUFFER="$BUFFER"

    if (( !_SHELLSUGGEST_ENABLED )) || [[ -z "$BUFFER" ]] || _shellsuggest_buffer_limit_exceeded; then
        _shellsuggest_clear_suggestion
        return
    fi

    _shellsuggest_refresh_suggestion_for_buffer
    _shellsuggest_query "$BUFFER" "$CURSOR"
}

# Accept full suggestion, otherwise let the original widget handle cursor motion.
_shellsuggest_accept_full() {
    local original_widget_name="$1"
    shift

    local -i max_cursor_pos=$#BUFFER

    if [[ "$KEYMAP" == "vicmd" ]]; then
        max_cursor_pos=$(( max_cursor_pos - 1 ))
    fi

    if (( max_cursor_pos < 0 )); then
        max_cursor_pos=0
    fi

    if (( $CURSOR != $max_cursor_pos || !$#POSTDISPLAY )); then
        return 1
    fi

    _shellsuggest_record_feedback true
    BUFFER="$BUFFER$POSTDISPLAY"
    _shellsuggest_clear_suggestion

    _shellsuggest_invoke_original_widget "$original_widget_name" "$@"
    _shellsuggest_clear_highlight

    if [[ "$KEYMAP" == "vicmd" ]]; then
        CURSOR=$(( ${#BUFFER} > 0 ? ${#BUFFER} - 1 : 0 ))
    else
        CURSOR=${#BUFFER}
    fi

    _SHELLSUGGEST_LAST_BUFFER="$BUFFER"
    zle -R
    return 0
}

_shellsuggest_accept() {
    _shellsuggest_accept_full _shellsuggest_orig_forward-char "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_forward-char "$@"
}

# Accept as much of the suggestion as the wrapped motion widget would consume.
_shellsuggest_accept_word_partial() {
    local original_widget_name="$1"
    shift

    local -i cursor_loc
    local original_buffer="$BUFFER"
    local original_suggestion="$_SHELLSUGGEST_SUGGESTION"
    local original_postdisplay="$POSTDISPLAY"

    if [[ -z "$original_suggestion" || -z "$original_postdisplay" ]]; then
        return 1
    fi

    BUFFER="$BUFFER$original_postdisplay"
    _shellsuggest_invoke_original_widget "$original_widget_name" "$@"

    cursor_loc=$CURSOR
    if [[ "$KEYMAP" == "vicmd" ]]; then
        cursor_loc=$(( cursor_loc + 1 ))
    fi

    if (( cursor_loc <= $#original_buffer )); then
        BUFFER="$original_buffer"
        POSTDISPLAY="$original_postdisplay"
        _shellsuggest_apply_highlight
        return 1
    fi

    _shellsuggest_record_feedback true
    BUFFER="${BUFFER[1,$cursor_loc]}"
    _SHELLSUGGEST_LAST_BUFFER="$BUFFER"

    if [[ "$original_suggestion" == "$BUFFER"* && "$original_suggestion" != "$BUFFER" ]]; then
        _SHELLSUGGEST_SUGGESTION="$original_suggestion"
        POSTDISPLAY="${original_suggestion#$BUFFER}"
        _shellsuggest_apply_highlight
    else
        _shellsuggest_clear_suggestion
    fi

    zle -R
    return 0
}

_shellsuggest_accept_word() {
    _shellsuggest_accept_word_partial _shellsuggest_orig_forward-word "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_forward-word "$@"
}

# Clear suggestion
_shellsuggest_clear() {
    [[ -n "$_SHELLSUGGEST_SUGGESTION" ]] && _shellsuggest_record_feedback false
    _shellsuggest_clear_suggestion
    zle -R
}

_shellsuggest_fetch() {
    local enabled_state=$_SHELLSUGGEST_ENABLED

    if [[ -z "$BUFFER" ]] || _shellsuggest_buffer_limit_exceeded; then
        _shellsuggest_clear_suggestion
        zle -R
        return 0
    fi

    _SHELLSUGGEST_ENABLED=1
    _shellsuggest_query "$BUFFER" "$CURSOR" || {
        _SHELLSUGGEST_ENABLED=$enabled_state
        return 1
    }
    _SHELLSUGGEST_ENABLED=$enabled_state
    zle -R
}

_shellsuggest_disable() {
    _SHELLSUGGEST_ENABLED=0
    _shellsuggest_clear_suggestion
    zle -R
}

_shellsuggest_enable() {
    _SHELLSUGGEST_ENABLED=1
    _shellsuggest_fetch || zle -R
}

_shellsuggest_toggle() {
    if (( _SHELLSUGGEST_ENABLED )); then
        _shellsuggest_disable
    else
        _shellsuggest_enable
    fi
}

# Wrap accept-line
_shellsuggest_execute() {
    _shellsuggest_clear_suggestion
    _shellsuggest_invoke_original_widget _shellsuggest_orig_accept-line "$@" || zle .accept-line
}

# preexec hook: capture the command and cwd before execution so `cd` is
# recorded against the directory it was actually run from.
_shellsuggest_preexec() {
    _SHELLSUGGEST_PENDING_COMMAND="$1"
    _SHELLSUGGEST_PENDING_CWD="$PWD"
}

# precmd hook: record the pending command with its final exit status
_shellsuggest_precmd() {
    local last_exit=$?
    local last_cmd="$_SHELLSUGGEST_PENDING_COMMAND"
    local command_cwd="$_SHELLSUGGEST_PENDING_CWD"

    if (( last_exit == 0 )) && [[ -n "$last_cmd" ]] && [[ -n "$command_cwd" ]]; then
        local escaped_cmd
        local escaped_cwd
        local escaped_session_id
        local message
        _SHELLSUGGEST_LAST_COMMAND="$last_cmd"
        escaped_cmd=$(_shellsuggest_line_escape "$last_cmd")
        escaped_cwd=$(_shellsuggest_line_escape "$command_cwd")
        escaped_session_id=$(_shellsuggest_line_escape "$_SHELLSUGGEST_SESSION_ID")
        message=$(_shellsuggest_join_fields "r" "$last_exit" "0" "$escaped_session_id" "$escaped_cwd" "$escaped_cmd")
        _shellsuggest_send_message "$message" >/dev/null 2>&1
    fi

    _SHELLSUGGEST_PENDING_COMMAND=""
    _SHELLSUGGEST_PENDING_CWD=""
    _SHELLSUGGEST_LAST_BUFFER=""
    _shellsuggest_clear_suggestion

    if [[ -z "$_SHELLSUGGEST_MANUAL_REBIND" ]]; then
        _shellsuggest_bind_widgets
    fi
}

# Wrap movement widgets so existing keymaps keep working.
_shellsuggest_wrap_widget() {
    local widget="$1"
    local backup="$2"
    local wrapper="$3"
    local current_widget="$widgets[$widget]"
    local backup_fn

    if [[ "$current_widget" == "user:${wrapper}" ]]; then
        return 0
    fi

    case "$current_widget" in
        user:*)
            zle -N "$backup" "${current_widget#*:}"
            ;;
        builtin)
            backup_fn="_shellsuggest_builtin_${${widget//-/_}//./_}"
            eval "${backup_fn}() { zle .${(q)widget} -- \"\$@\" }"
            zle -N "$backup" "$backup_fn"
            ;;
        *)
            zle -A "$widget" "$backup" 2>/dev/null || return 0
            ;;
    esac

    zle -N "$widget" "$wrapper"
}

_shellsuggest_forward_char() {
    _shellsuggest_accept_full _shellsuggest_orig_forward-char "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_forward-char "$@"
}

_shellsuggest_vi_forward_char() {
    _shellsuggest_accept_full _shellsuggest_orig_vi_forward-char "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_vi_forward-char "$@"
}

_shellsuggest_end_of_line() {
    _shellsuggest_accept_full _shellsuggest_orig_end-of-line "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_end-of-line "$@"
}

_shellsuggest_vi_end_of_line() {
    _shellsuggest_accept_full _shellsuggest_orig_vi-end-of-line "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_vi-end-of-line "$@"
}

_shellsuggest_forward_word() {
    _shellsuggest_accept_word_partial _shellsuggest_orig_forward-word "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_forward-word "$@"
}

_shellsuggest_emacs_forward_word() {
    _shellsuggest_accept_word_partial _shellsuggest_orig_emacs-forward-word "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_emacs-forward-word "$@"
}

_shellsuggest_vi_forward_word() {
    _shellsuggest_accept_word_partial _shellsuggest_orig_vi-forward-word "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_vi-forward-word "$@"
}

_shellsuggest_vi_forward_word_end() {
    _shellsuggest_accept_word_partial _shellsuggest_orig_vi-forward-word-end "$@" || _shellsuggest_invoke_original_widget _shellsuggest_orig_vi-forward-word-end "$@"
}

_shellsuggest_bracketed_paste() {
    _shellsuggest_clear_suggestion
    _shellsuggest_invoke_original_widget _shellsuggest_orig_bracketed-paste "$@"
}

_shellsuggest_cycle_next() {
    _shellsuggest_cycle next
}

_shellsuggest_cycle_prev() {
    _shellsuggest_cycle prev
}

_shellsuggest_bind_word_keys() {
    local keymap="$1"
    local widget="$2"

    [[ -n "${terminfo[kRIT5]}" ]] && bindkey -M "$keymap" "${terminfo[kRIT5]}" "$widget"
    [[ -n "${terminfo[kRIT3]}" ]] && bindkey -M "$keymap" "${terminfo[kRIT3]}" "$widget"
    bindkey -M "$keymap" '^[[1;5C' "$widget"
    bindkey -M "$keymap" '^[[1;3C' "$widget"
    bindkey -M "$keymap" '^[[5C' "$widget"
    bindkey -M "$keymap" '^[f' "$widget"
}

_shellsuggest_bind_widgets() {
    _shellsuggest_wrap_widget forward-char _shellsuggest_orig_forward-char _shellsuggest_forward_char
    _shellsuggest_wrap_widget vi-forward-char _shellsuggest_orig_vi-forward-char _shellsuggest_vi_forward_char
    _shellsuggest_wrap_widget end-of-line _shellsuggest_orig_end-of-line _shellsuggest_end_of_line
    _shellsuggest_wrap_widget vi-end-of-line _shellsuggest_orig_vi-end-of-line _shellsuggest_vi_end_of_line
    _shellsuggest_wrap_widget forward-word _shellsuggest_orig_forward-word _shellsuggest_forward_word
    _shellsuggest_wrap_widget emacs-forward-word _shellsuggest_orig_emacs-forward-word _shellsuggest_emacs_forward_word
    _shellsuggest_wrap_widget vi-forward-word _shellsuggest_orig_vi-forward-word _shellsuggest_vi_forward_word
    _shellsuggest_wrap_widget vi-forward-word-end _shellsuggest_orig_vi-forward-word-end _shellsuggest_vi_forward_word_end
    _shellsuggest_wrap_widget bracketed-paste _shellsuggest_orig_bracketed-paste _shellsuggest_bracketed_paste
    _shellsuggest_wrap_widget accept-line _shellsuggest_orig_accept-line _shellsuggest_execute
}

# Register widgets
zle -N _shellsuggest_accept
zle -N _shellsuggest_accept_word
zle -N _shellsuggest_clear
zle -N _shellsuggest_execute
zle -N _shellsuggest_fetch
zle -N _shellsuggest_disable
zle -N _shellsuggest_enable
zle -N _shellsuggest_toggle
zle -N _shellsuggest_forward_char
zle -N _shellsuggest_vi_forward_char
zle -N _shellsuggest_end_of_line
zle -N _shellsuggest_vi_end_of_line
zle -N _shellsuggest_forward_word
zle -N _shellsuggest_emacs_forward_word
zle -N _shellsuggest_vi_forward_word
zle -N _shellsuggest_vi_forward_word_end
zle -N _shellsuggest_bracketed_paste
zle -N _shellsuggest_cycle_next
zle -N _shellsuggest_cycle_prev

# zsh-autosuggestions compatibility widgets
zle -N autosuggest-accept _shellsuggest_accept
zle -N autosuggest-clear _shellsuggest_clear
zle -N autosuggest-execute _shellsuggest_execute
zle -N autosuggest-fetch _shellsuggest_fetch
zle -N autosuggest-disable _shellsuggest_disable
zle -N autosuggest-enable _shellsuggest_enable
zle -N autosuggest-toggle _shellsuggest_toggle

_shellsuggest_bind_widgets

# Bind keys
bindkey -M emacs '\e' _shellsuggest_clear
bindkey -M emacs '^[n' _shellsuggest_cycle_next
bindkey -M emacs '^[p' _shellsuggest_cycle_prev
bindkey -M viins '^[n' _shellsuggest_cycle_next
bindkey -M viins '^[p' _shellsuggest_cycle_prev

_shellsuggest_bind_word_keys emacs emacs-forward-word
_shellsuggest_bind_word_keys viins forward-word

# Hook into line editing
autoload -Uz add-zle-hook-widget
add-zle-hook-widget zle-line-pre-redraw _shellsuggest_suggest

# Hook into precmd
autoload -Uz add-zsh-hook
add-zsh-hook preexec _shellsuggest_preexec
add-zsh-hook precmd _shellsuggest_precmd

# Start coproc connection to the Rust query engine
_shellsuggest_start_coproc
