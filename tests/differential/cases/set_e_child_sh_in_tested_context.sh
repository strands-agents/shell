printf 'set -e\necho building\nfalse\necho "build ok"\n' > build.sh
sh -c 'set -e; false; echo child-continued' || echo child-failed
! sh -c 'set -e; false; echo child-negated'
sh build.sh || echo build-failed
sh build.sh && echo deploying
echo after
