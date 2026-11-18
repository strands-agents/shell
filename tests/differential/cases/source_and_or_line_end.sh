printf 'false &&\necho AND_SKIPPED\ntrue ||\necho OR_SKIPPED\ntrue &&\n  echo AND_RAN\nfalse ||\n\n  echo OR_RAN\n' > and_or.sh
. ./and_or.sh
echo "dot=$?"
source ./and_or.sh
echo "source=$?"
sh ./and_or.sh
echo "sh=$?"
printf 'false &&\necho skipped\n' > guard.sh
. ./guard.sh
echo "guard=$?"
