// `*dst = *src` copies a whole object through two pointers, and a member that
// is itself a struct is assigned as one object too.
//@ exit-code: 24

struct Inner {
    int a;
    int b;
};

struct Outer {
    struct Inner in;
    int tail;
};

void copy(struct Inner *dst, struct Inner *src) {
    *dst = *src;
}

int main() {
    struct Inner x;
    x.a = 3;
    x.b = 4;

    struct Outer o;
    o.in = x;
    o.tail = 5;

    struct Inner y;
    copy(&y, &o.in);

    struct Inner list[3];
    int i;
    for (i = 0; i < 3; i = i + 1) {
        list[i] = x;
        list[i].a = i;
    }
    list[2] = list[0];

    return o.in.a + o.in.b + o.tail + y.a + y.b
         + list[0].a + list[1].a + list[2].a + list[2].b;
}
