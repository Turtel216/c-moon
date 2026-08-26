// A struct inside a struct: the member's offset is the sum of the offsets
// along the way, so the whole chain is one address.
//@ exit-code: 55

struct Inner {
    int a;
    char c;
};

struct Outer {
    struct Inner in;
    long int n;
};

int main() {
    struct Outer o;
    o.in.a = 20;
    o.in.c = 5;
    o.n = 30;

    struct Outer *p = &o;
    p->in.a = p->in.a + 0;

    return o.in.a + o.in.c + (int)o.n;
}
