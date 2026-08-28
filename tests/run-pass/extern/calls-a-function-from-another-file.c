// An `extern` function is defined in another translation unit; the linker is
// what joins the two.
//@ aux-build: arithmetic.c
//@ exit-code: 21

extern int triple(int x);

int main() {
    return triple(7);
}
