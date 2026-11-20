shopt -s expand_aliases 2>/dev/null
alias foo=bar
[[ -n x && foo == foo ]] && echo no-alias-inside
