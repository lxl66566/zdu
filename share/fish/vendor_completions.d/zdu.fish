complete -c zdu -s d -l depth -d 'Depth to show' -r
complete -c zdu -s T -l threads -d 'Number of threads to use' -r
complete -c zdu -l config -d 'Specify a config file to use' -r -F
complete -c zdu -s n -l number-of-lines -d 'Display the \'n\' largest entries. (Default is terminal_height)' -r
complete -c zdu -s X -l ignore-directory -d 'Exclude any file or directory with this path' -r -F
complete -c zdu -s I -l ignore-all-in-file -d 'Exclude any file or directory with a regex matching that listed in this file, the file entries will be added to the ignore regexs provided by --invert_filter' -r -F
complete -c zdu -s z -l min-size -d 'Minimum size file to include in output' -r
complete -c zdu -s v -l invert-filter -d 'Exclude filepaths matching this regex. To ignore png files type: -v "\\.png$"' -r
complete -c zdu -s e -l filter -d 'Only include filepaths matching this regex. For png files type: -e "\\.png$"' -r
complete -c zdu -s w -l terminal-width -d 'Specify width of output overriding the auto detection of terminal width' -r
complete -c zdu -s o -l output-format -d 'Changes output display size. si will print sizes in powers of 1000. b k m g t kb mb gb tb will print the whole tree in that size' -r -f -a "si\t'SI prefix (powers of 1000)'
b\t'byte (B)'
k\t'kibibyte (KiB)'
m\t'mebibyte (MiB)'
g\t'gibibyte (GiB)'
t\t'tebibyte (TiB)'
kb\t'kilobyte (kB)'
mb\t'megabyte (MB)'
gb\t'gigabyte (GB)'
tb\t'terabyte (TB)'"
complete -c zdu -s S -l stack-size -d 'Deprecated. The walker no longer recurses so a custom stack size is unnecessary. Accepted for compatibility but the value is ignored' -r
complete -c zdu -s M -l mtime -d '+/-n matches files modified more/less than n days ago , and n matches files modified exactly n days ago, days are rounded down.That is +n => (−∞, curr−(n+1)), n => [curr−(n+1), curr−n), and -n => (𝑐𝑢𝑟𝑟−𝑛, +∞)' -r
complete -c zdu -s A -l atime -d 'just like -mtime, but based on file access time' -r
complete -c zdu -s y -l ctime -d 'just like -mtime, but based on file change time' -r
complete -c zdu -l files0-from -d 'Read NUL-terminated paths from FILE (use `-` for stdin)' -r -F
complete -c zdu -l files-from -d 'Read newline-terminated paths from FILE (use `-` for stdin)' -r -F
complete -c zdu -l collapse -d 'Keep these directories collapsed' -r -F
complete -c zdu -s m -l filetime -d 'Directory \'size\' is max filetime of child files instead of disk size. while a/c/m for last accessed/changed/modified time' -r -f -a "a\t'last accessed time'
c\t'last changed time'
m\t'last modified time'"
complete -c zdu -s p -l full-paths -d 'Subdirectories will not have their path shortened'
complete -c zdu -s L -l dereference-links -d 'dereference sym links - Treat sym links as directories and go into them'
complete -c zdu -s x -l limit-filesystem -d 'Only count the files and directories on the same filesystem as the supplied directory'
complete -c zdu -s s -l apparent-size -d 'Use file length instead of blocks (on Windows the default mode is the on-disk size, which equals file length for plain files)'
complete -c zdu -s r -l reverse -d 'Print tree upside down (biggest highest)'
complete -c zdu -s c -l no-colors -d 'No colors will be printed (Useful for commands like: watch)'
complete -c zdu -s C -l force-colors -d 'Force colors print'
complete -c zdu -l dim -d 'Dim the percent bars (grey) to reduce brightness on dark terminals'
complete -c zdu -s b -l no-percent-bars -d 'No percent bars or percentages will be displayed'
complete -c zdu -s B -l bars-on-right -d 'percent bars moved to right side of screen'
complete -c zdu -s R -l screen-reader -d 'For screen readers. Removes bars. Adds new column: depth level (May want to use -p too for full path)'
complete -c zdu -l skip-total -d 'No total row will be displayed'
complete -c zdu -s f -l filecount -d 'Directory \'size\' is number of child files instead of disk size'
complete -c zdu -s i -l ignore-hidden -d 'Do not display hidden files'
complete -c zdu -s t -l file-types -d 'show only these file types'
complete -c zdu -s P -l no-progress -d 'Disable the progress indication'
complete -c zdu -l print-errors -d 'Print path with errors'
complete -c zdu -s D -l only-dir -d 'Only directories will be displayed'
complete -c zdu -s F -l only-file -d 'Only files will be displayed. (Finds your largest files)'
complete -c zdu -s j -l output-json -d 'Output the directory tree as json to the current directory'
complete -c zdu -s h -l help -d 'Print help (see more with \'--help\')'
complete -c zdu -s V -l version -d 'Print version'
