# known-difference: a partly quoted heredoc delimiter like E\OT isn't recognized, so the body runs past it
v=val
cat <<"EOT"
double $v
EOT
cat <<E\OT
escaped $v
EOT
