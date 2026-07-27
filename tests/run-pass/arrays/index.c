// Array elements are addressable by constant and by variable index.
//@ exit-code: 30

int main() {
    int arr[3];
    int i = 1;
    arr[0] = 10;
    arr[i] = 20;
    int x = arr[0] + arr[1];
    return x;
}
