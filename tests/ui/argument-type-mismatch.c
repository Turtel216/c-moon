// An argument whose type the parameter does not accept.

int twice(int n) {
    return n + n;
}

int main() {
    int value = 1;
    return twice(&value);
}
