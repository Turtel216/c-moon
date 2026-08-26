// Where a member sits is decided by the ABI, not by the language, so the
// offsets are checked directly: an address cast to an integer is a number, and
// the distance between two of them is the offset. Running this under GCC too is
// what makes it an assertion about the ABI rather than about this compiler.
//
//   struct Mixed { char c; int n; char d; long int l; };
//     c at 0, n at 4 (three bytes of padding), d at 8, l at 16
//
//@ exit-code: 36

struct Mixed {
    char c;
    int n;
    char d;
    long int l;
};

struct Small {
    int n;
    char c;
};

int main() {
    struct Mixed m;
    long int base = (long int)&m;

    int offsets = (int)((long int)&m.c - base)
                + (int)((long int)&m.n - base)
                + (int)((long int)&m.d - base)
                + (int)((long int)&m.l - base);

    // The distance between two elements is the size of one, which is rounded
    // up to the struct's alignment: five bytes of members, eight of object.
    struct Small items[2];
    int stride = (int)((long int)&items[1] - (long int)&items[0]);

    return offsets + stride;
}
