echo "${unset_a:=assigned}"
echo "$unset_a"
empty=
echo "[${empty:-fallback}] [${empty-kept}]"
