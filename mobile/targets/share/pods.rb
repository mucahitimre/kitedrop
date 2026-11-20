# Runs inside `target 'share' do ... end` of the generated Podfile (paths relative to ios/).
# Links only the pure-native core: no React Native or Expo in the share extension (design §4).
pod 'KitedropCore', :path => '../native/ios/KitedropCore'
