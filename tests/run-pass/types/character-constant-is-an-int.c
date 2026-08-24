// A character constant has type `int` in C, not `char`, so it holds values a
// `char` could not and is not truncated where it is written.
//@ exit-code: 200

int main() {
    int big = 'a' * 2;
    char truncated = big;
    // 194 as an `int`, but -62 once it has been squeezed into a `char`.
    if (truncated > 0) {
        return 1;
    }
    return big + 6;
}
