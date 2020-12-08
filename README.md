# epubsearch

Search an epub document for matches of a regular expression, and print
the matches to the terminal in brilliant technicolor.

## Usage 

```
Usage: epubsearch <regex> [<file_names...>] [-c] [-q] [-i] [-w] [--color <color>]

Options:
  -c, --count       print the number of matching paragraphs
  -q, --quiet       produce no output
  -i, --ignore-case do case insensitive search
  -w, --word-regexp find matches surrounded by word boundaries
  --color           whether to print results in color. options: always, auto,
                    never
  --help            display usage information
```
