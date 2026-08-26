// A struct may hold a pointer to its own type: a pointer is a machine word
// whatever it points at, so the tag needs no layout for it.
//@ exit-code: 200

struct Node {
    unsigned char byte;
    struct Node *next;
    long int value;
};

long int walk(struct Node *n, int steps) {
    long int total = 0;
    int i;
    for (i = 0; i < steps; i = i + 1) {
        total = total + n->value + n->byte;
        n = n->next;
    }
    return total;
}

int main() {
    struct Node a;
    struct Node b;

    a.byte = 200;
    a.value = 1000;
    a.next = &b;

    b.byte = 55;
    b.value = 2000;
    b.next = &b;

    struct Node c = a;
    c.value = 1;

    return (int)(walk(&a, 2) + walk(&c, 1)) - 3000;
}
