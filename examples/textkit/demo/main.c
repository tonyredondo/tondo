/* A C program that uses the Tondo library: `sh demo/build.sh` builds both. */

#include <stdio.h>

#include "textkit.h"

/* What `textkit_words` calls for each word: prints those said more than once. */
static void print_repeated(char *word, long count, void *context) {
    long *printed = context;
    if (count > 1) {
        printf("  %-10s %ld\n", word, count);
        *printed += 1;
    }
}

int main(void) {
    const char *title = "Crème Brûlée, 2 ways!";
    char slug[32], small[8];
    size_t length = textkit_slug(title, slug, sizeof slug);
    printf("slug of \"%s\": %s (%zu bytes)\n", title, slug, length);
    if (textkit_slug(title, small, sizeof small) >= sizeof small) {
        printf("in 8 bytes it is cut: %s\n", small);
    }

    printf("from \"kitten\" to \"sitting\": %ld changes\n", textkit_distance("kitten", "sitting"));

    const char *speech = "To be, or not to be: that is the question.\n"
                         "Whether 'tis nobler in the mind to suffer\n";
    TextkitStats stats = textkit_stats(speech);
    printf("%ld characters, %ld words, %ld lines; said more than once:\n",
           stats.characters, stats.words, stats.lines);

    long printed = 0;
    textkit_words(speech, print_repeated, &printed);
    printf("that is %ld words\n", printed);
    return 0;
}
