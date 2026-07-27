// A call passing two register arguments.
//@ exit-code: 2

int add(int a, int b) {
    return a + b;
}

int main() {
    int a = 1;
    int b = 1;

    return add(a, b);
}
