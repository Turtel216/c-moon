// `!` answers 1 for zero and 0 for anything else, so `!!x` asks whether `x`
// is non-zero.
//@ exit-code: 5

int main() {
    int zero = 0;
    int five = 5;
    return !zero * 4 + !five * 2 + !!five;
}
