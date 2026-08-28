// A `char` argument and a `long int` result cross the boundary: the argument
// is passed in the low byte of the ABI register and the answer comes back in
// the whole of RAX, too wide for the `int` it is then narrowed to.
//@ aux-build: arithmetic.c
//@ exit-code: 65

extern long widen(char c);

int main() {
    long scaled = widen('A');
    return scaled / 1000000000;
}
