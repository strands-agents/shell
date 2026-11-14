# known-difference: every variable reaches the commands the shell runs, exported or not; fixed by #285
plain=1
env | grep '^plain=' || echo "plain hidden"
export gone=1
unset gone
gone=2
env | grep '^gone=' || echo "gone hidden"
export dropped=1
export -n dropped
env | grep '^dropped=' || echo "dropped hidden"
readonly fixed=1
env | grep '^fixed=' || echo "fixed hidden"
shown=1
sh -c 'echo "child sees [$shown]"'
