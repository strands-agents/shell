# known-difference: pipeline stages expand their words in the parent shell
RANDOM=42
echo $RANDOM | cat >/dev/null
echo $RANDOM
