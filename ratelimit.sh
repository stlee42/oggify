split -l 2 trackids OGGIFY_PREFIX_
for i in OGGIFY_PREFIX_*; do
	oggify "$userid" "$password" < "$i"
	echo sleeping for two minutes
	sleep 120
done
rm OGGIFY_PREFIX_*
