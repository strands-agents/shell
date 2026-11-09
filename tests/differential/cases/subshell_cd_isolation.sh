mkdir sub
before=$(pwd)
(cd sub && echo "inside $(basename "$(pwd)")")
[ "$(pwd)" = "$before" ] && echo "outside unchanged"
