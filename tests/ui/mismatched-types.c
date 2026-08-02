// An initializer whose type is not the declared one.

int main() {
    int value = 1;
    int *pointer = &value;
    int copy = pointer;
    return copy;
}
