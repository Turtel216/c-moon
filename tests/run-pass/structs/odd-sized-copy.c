// An object whose size is not a multiple of the machine word is copied by the
// widest moves that fit and no wider. The three-byte struct is followed by a
// member the ABI puts at offset 4, so a copy that rounded up to four or eight
// bytes would overwrite it.
//@ exit-code: 106

struct Three {
    char a;
    char b;
    char c;
};

struct Holder {
    struct Three three;
    int sentinel;
};

int main() {
    struct Three source;
    source.a = 1;
    source.b = 2;
    source.c = 3;

    struct Holder holder;
    holder.sentinel = 100;
    holder.three = source;

    return holder.three.a + holder.three.b + holder.three.c + holder.sentinel;
}
