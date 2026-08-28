// A pointer holds an address rather than a number, so no mask applies to it.

int main() {
    int value = 1;
    int* pointer = &value;
    return pointer & 1;
}
