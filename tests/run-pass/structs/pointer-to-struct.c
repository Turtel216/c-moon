// `->` reads a member through a pointer, which is how a struct crosses a call
// boundary here.
//@ exit-code: 30

struct Counter {
    int value;
    int step;
};

void advance(struct Counter *c) {
    c->value = c->value + c->step;
}

int total(struct Counter *c) {
    return c->value;
}

int main() {
    struct Counter c;
    c.value = 0;
    c.step = 10;

    advance(&c);
    advance(&c);
    advance(&c);

    return total(&c);
}
