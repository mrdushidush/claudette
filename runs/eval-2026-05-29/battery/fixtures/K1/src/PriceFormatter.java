package shop;

public class PriceFormatter {
    public String format(double amount) {
        return String.format("$%.2f", amount);
    }
}
