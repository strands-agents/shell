false
echo "false=$?"
(exit 3)
echo "subshell=$?"
! true
echo "negated=$?"
exit 5
