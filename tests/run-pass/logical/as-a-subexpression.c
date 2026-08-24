// A logical operator is an ordinary `int` expression: it may be indexed with,
// passed, and converted like any other, even though computing it branched.
//@ exit-code: 42

int twice(int value) {
    return value + value;
}

int main() {
    int table[2];
    table[0] = 5;
    table[1] = 16;

    int a = 3;
    int b = 0;

    int chosen = table[a && !b];
    int passed = twice(a || b);
    long int widened = a && a;

    return chosen * 2 + passed * 4 + (int)widened * 2;
}
