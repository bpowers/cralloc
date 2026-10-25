#!/usr/bin/env bash
# Prints the free blocks of 2 MiB or more the kernel holds (orders 9 and above of
# /proc/buddyinfo, summed over zones, counted in 2 MiB blocks): how many huge-page faults can
# be served without compaction right now.
awk '{ n = 0; for (i = 14; i <= NF; i++) n += $i * 2 ^ (i - 14); t += n } END { print t }' /proc/buddyinfo
