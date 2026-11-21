set -e
printf 'false\necho in-lib\n' > lib.sh
. ./lib.sh || echo caught-dot
source ./lib.sh && echo ok-source
echo after
