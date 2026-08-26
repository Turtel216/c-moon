// An array inside a struct, indexed by a constant and by a variable.
//@ exit-code: 60

struct Row {
    int head;
    int data[4];
};

int main() {
    struct Row r;
    r.head = 10;

    int i;
    for (i = 0; i < 4; i = i + 1) {
        r.data[i] = i * 5;
    }

    return r.head + r.data[0] + r.data[1] + r.data[2] + r.data[3] + 20;
}
