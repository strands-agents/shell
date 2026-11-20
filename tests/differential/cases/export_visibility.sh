plain=1
env | grep '^plain=' || echo "plain hidden"
echo "plain=$plain"
prefix=1 env | grep '^prefix='
env | grep '^prefix=' || echo "prefix gone"
later=1
export later
env | grep '^later='
export early
env | grep '^early=' || echo "early unset"
early=2
env | grep '^early='
export dropped=1
export -n dropped
env | grep '^dropped=' || echo "dropped hidden"
echo "dropped=$dropped"
export gone=1
unset gone
gone=2
env | grep '^gone=' || echo "gone hidden"
readonly fixed=1
env | grep '^fixed=' || echo "fixed hidden"
export kept=1
f() { local kept; unset kept; }
f
env | grep '^kept='
g() { export inner=1; }
g
env | grep '^inner='
shown=1
sh -c 'echo "child sees [$shown]"'
export shown
sh -c 'echo "child sees [$shown]"'
hidden=1
(echo "subshell sees [$hidden]")
echo "substitution sees [$(echo "$hidden")]"
