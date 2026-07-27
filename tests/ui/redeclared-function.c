// Two definitions of the same function.

int f() { return 1; }
int f() { return 2; }

int main() {
    return f();
}
